#[path = "operation_risk_declaration/support.rs"]
mod operation_risk_support;
mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    error::ApplicationErrorCode,
    operation_risk_declaration::{
        DeclareProviderImplementationRiskRequest, OperationRiskDeclarationService,
    },
};
use chatoms_domain::{HighRiskCategory, OperationRiskKind, TaskState};
use chatoms_ports::{error::FailureCategory, repository::HighRiskApprovalRecord};

use operation_risk_support::fixture;

#[test]
fn declaration_service_persists_explicit_empty_without_other_execution_side_effects() {
    let (mut repository, mut filesystem, task) = fixture();
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };

    let declaration = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect("declare explicit empty assessment");

    assert!(declaration.risk_categories.is_empty());
    assert_eq!(
        declaration.record.operation_kind,
        OperationRiskKind::ProviderImplementation
    );
    assert_eq!(
        repository.tasks[&task.id()].state(),
        TaskState::AwaitingDesignApproval
    );
    assert!(repository.consents.is_empty());
    assert_eq!(repository.operation_risk_declarations.len(), 1);
    assert_eq!(repository.calls.last(), Some(&"declare_operation_risk"));
}

#[test]
fn declaration_service_requires_each_selected_high_risk_approval() {
    let (mut repository, mut filesystem, task) = fixture();
    let foreign_task_id = chatoms_domain::TaskId::new();
    repository.high_risk_approvals.insert(
        (
            foreign_task_id,
            task.version(),
            HighRiskCategory::ArchitectureChange,
        ),
        HighRiskApprovalRecord {
            task_id: foreign_task_id,
            approved_task_version: task.version(),
            risk_category: HighRiskCategory::ArchitectureChange,
            approved_at_ms: 190,
        },
    );
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: vec![HighRiskCategory::ArchitectureChange],
        declared_at_ms: 200,
    };

    let error = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect_err("missing category approval must fail closed");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_rejects_project_identity_mismatch_without_storage() {
    let (mut repository, mut filesystem, task) = fixture();
    filesystem
        .identities
        .get_mut(Path::new("C:/project"))
        .expect("project identity")
        .file_id_hex = "ffffffffffffffffffffffffffffffff".to_owned();
    let mismatch_request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };

    let mismatch = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(mismatch_request)
        .expect_err("project identity mismatch must fail closed");
    assert_eq!(mismatch.code(), ApplicationErrorCode::Conflict);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_preserves_identity_inspection_failure_without_storage() {
    let (mut repository, mut filesystem, task) = fixture();
    filesystem.failures.insert(
        PathBuf::from("C:/managed/worktree"),
        FailureCategory::StorageUnavailable,
    );
    let failure_request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };
    let failure = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(failure_request)
        .expect_err("worktree identity inspection failure must fail closed");
    assert_eq!(failure.code(), ApplicationErrorCode::StorageUnavailable);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_rejects_stale_version_before_identity_inspection() {
    let (mut repository, mut filesystem, task) = fixture();
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version() - 1,
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };

    let error = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect_err("stale version must fail closed");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_rejects_wrong_state_without_storage() {
    let (mut repository, mut filesystem, mut task) = fixture();
    task.transition_to(TaskState::Implementing, 60)
        .expect("transition fixture to Implementing");
    repository.tasks.insert(task.id(), task.clone());
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };

    let error = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect_err("wrong state must fail closed");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_rejects_missing_active_lease_without_storage() {
    let (mut repository, mut filesystem, task) = fixture();
    repository.active_lease = None;
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };

    let error = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect_err("missing lease must fail closed");

    assert_eq!(error.code(), ApplicationErrorCode::ActiveTaskConflict);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_preserves_repository_failure_without_storage() {
    let (mut repository, mut filesystem, task) = fixture();
    repository.fail_on = Some((
        "declare_operation_risk",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: Vec::new(),
        declared_at_ms: 200,
    };

    let error = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect_err("repository failure must not become approval-required");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(repository.operation_risk_declarations.is_empty());
}

#[test]
fn declaration_service_persists_only_the_explicitly_selected_approved_categories() {
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
    let request = DeclareProviderImplementationRiskRequest {
        task_id: task.id(),
        expected_version: task.version(),
        risk_categories: vec![HighRiskCategory::ArchitectureChange],
        declared_at_ms: 200,
    };

    let declaration = OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(request)
        .expect("declare selected category");

    assert_eq!(
        declaration.risk_categories,
        vec![HighRiskCategory::ArchitectureChange]
    );
}
