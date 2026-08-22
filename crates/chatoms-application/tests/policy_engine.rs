#[path = "policy_engine/fail_closed.rs"]
mod fail_closed;
#[path = "operation_risk_declaration/support.rs"]
mod operation_risk_support;
mod support;

use chatoms_application::{
    operation_risk_declaration::{
        DeclareProviderImplementationRiskRequest, OperationRiskDeclarationService,
    },
    policy_engine::{
        PolicyDecision, PolicyDenialReason, PolicyEngine, PolicyEvaluationRequest, PolicyOperation,
    },
};
use chatoms_domain::{HighRiskCategory, OperationRiskKind, TaskId};
use chatoms_ports::{
    provider_implementation_policy::ProviderImplementationPolicyBinding,
    repository::HighRiskApprovalRecord,
};

use operation_risk_support::fixture;

fn evaluate(
    repository: &mut support::FakeRepository,
    filesystem: &mut operation_risk_support::FakeFilesystem,
    task_id: TaskId,
    version: u64,
) -> Result<PolicyDecision, chatoms_application::error::ApplicationError> {
    PolicyEngine::new(repository, filesystem).evaluate(PolicyEvaluationRequest {
        task_id,
        expected_version: version,
        operation: PolicyOperation::ProviderImplementation,
    })
}

fn declare(
    repository: &mut support::FakeRepository,
    filesystem: &mut operation_risk_support::FakeFilesystem,
    task_id: TaskId,
    version: u64,
    categories: Vec<HighRiskCategory>,
) {
    OperationRiskDeclarationService::new(repository, filesystem)
        .declare_provider_implementation_risk(DeclareProviderImplementationRiskRequest {
            task_id,
            expected_version: version,
            risk_categories: categories,
            declared_at_ms: 200,
        })
        .expect("persist declaration fixture");
}

fn assert_denied(decision: PolicyDecision, expected: PolicyDenialReason) {
    match decision {
        PolicyDecision::Denied(reason) => assert_eq!(reason, expected),
        PolicyDecision::Authorized(_) | PolicyDecision::AssessmentRequired => {
            panic!("expected closed denial")
        }
    }
}

#[test]
fn explicit_empty_declaration_authorizes_provider_implementation() {
    let (mut repository, mut filesystem, task) = fixture();
    declare(
        &mut repository,
        &mut filesystem,
        task.id(),
        task.version(),
        Vec::new(),
    );

    let decision = evaluate(&mut repository, &mut filesystem, task.id(), task.version())
        .expect("evaluate policy");
    let PolicyDecision::Authorized(permit) = decision else {
        panic!("explicit empty assessment must authorize");
    };
    let declaration = repository
        .operation_risk_declarations
        .get(&(
            task.id(),
            task.version(),
            OperationRiskKind::ProviderImplementation,
        ))
        .expect("declaration");
    assert_eq!(permit.task_id(), task.id());
    assert_eq!(permit.approved_task_version(), task.version());
    assert_eq!(
        permit.operation_kind(),
        OperationRiskKind::ProviderImplementation
    );
    assert_eq!(
        permit.target_identity_digest(),
        declaration.record.target_identity_digest
    );
    assert!(
        permit.matches_worktree_object_identity(
            "0000000000000002",
            "22222222222222222222222222222222"
        )
    );
    assert!(
        !permit.matches_worktree_object_identity(
            "0000000000000002",
            "33333333333333333333333333333333"
        )
    );
}

#[test]
fn exact_selected_approvals_authorize_without_expanding_to_extra_approval() {
    let (mut repository, mut filesystem, task) = fixture();
    for category in [
        HighRiskCategory::ArchitectureChange,
        HighRiskCategory::DataMigration,
    ] {
        repository.high_risk_approvals.insert(
            (task.id(), task.version(), category),
            HighRiskApprovalRecord {
                task_id: task.id(),
                approved_task_version: task.version(),
                risk_category: category,
                approved_at_ms: 190,
            },
        );
    }
    declare(
        &mut repository,
        &mut filesystem,
        task.id(),
        task.version(),
        vec![HighRiskCategory::ArchitectureChange],
    );
    repository.calls.clear();

    assert!(matches!(
        evaluate(&mut repository, &mut filesystem, task.id(), task.version())
            .expect("evaluate exact approval"),
        PolicyDecision::Authorized(_)
    ));
    assert_eq!(
        repository
            .calls
            .iter()
            .filter(|call| **call == "get_high_risk_approval")
            .count(),
        1
    );
}

#[test]
fn missing_declaration_requires_assessment_but_empty_does_not() {
    let (mut repository, mut filesystem, task) = fixture();

    assert!(matches!(
        evaluate(&mut repository, &mut filesystem, task.id(), task.version())
            .expect("evaluate missing declaration"),
        PolicyDecision::AssessmentRequired
    ));
}

#[test]
fn unsupported_closed_operation_is_denied_without_authoritative_reads() {
    let (mut repository, mut filesystem, task) = fixture();
    repository.calls.clear();

    let decision = PolicyEngine::new(&mut repository, &mut filesystem)
        .evaluate(PolicyEvaluationRequest {
            task_id: task.id(),
            expected_version: task.version(),
            operation: PolicyOperation::Unsupported,
        })
        .expect("unsupported kind is a decision");

    assert_denied(decision, PolicyDenialReason::UnsupportedOperation);
    assert!(repository.calls.is_empty());
}

#[test]
fn evaluation_is_read_only_and_has_no_execution_collaborator() {
    let (mut repository, mut filesystem, task) = fixture();
    declare(
        &mut repository,
        &mut filesystem,
        task.id(),
        task.version(),
        Vec::new(),
    );
    repository.calls.clear();
    let task_before = repository.tasks[&task.id()].clone();
    let declarations_before = repository.operation_risk_declarations.clone();
    let consents_before = repository.consents.clone();

    assert!(matches!(
        evaluate(&mut repository, &mut filesystem, task.id(), task.version())
            .expect("read-only evaluation"),
        PolicyDecision::Authorized(_)
    ));
    assert_eq!(repository.tasks[&task.id()], task_before);
    assert_eq!(repository.operation_risk_declarations, declarations_before);
    assert_eq!(repository.consents, consents_before);
    assert!(
        repository
            .calls
            .iter()
            .all(|call| call.starts_with("get_") || *call == "active_lease")
    );
}
