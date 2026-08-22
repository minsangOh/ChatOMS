use std::path::{Path, PathBuf};

use chatoms_application::error::ApplicationErrorCode;
use chatoms_domain::{HighRiskCategory, TaskId, TaskState};
use chatoms_ports::{
    error::FailureCategory,
    repository::{HighRiskApprovalRecord, RepositoryErrorCode},
};

use super::*;

#[test]
fn stale_version_wrong_state_and_lease_mismatch_are_closed_denials() {
    let (mut repository, mut filesystem, task) = fixture();
    assert_denied(
        evaluate(
            &mut repository,
            &mut filesystem,
            task.id(),
            task.version() - 1,
        )
        .expect("stale policy request"),
        PolicyDenialReason::VersionMismatch,
    );

    let mut wrong_state = task.clone();
    wrong_state
        .transition_to(TaskState::Implementing, 60)
        .expect("wrong-state fixture");
    repository.tasks.insert(task.id(), wrong_state.clone());
    assert_denied(
        evaluate(
            &mut repository,
            &mut filesystem,
            task.id(),
            wrong_state.version(),
        )
        .expect("wrong-state request"),
        PolicyDenialReason::StateMismatch,
    );

    repository.tasks.insert(task.id(), task.clone());
    repository.active_lease = None;
    assert_denied(
        evaluate(&mut repository, &mut filesystem, task.id(), task.version())
            .expect("lease mismatch request"),
        PolicyDenialReason::LeaseMismatch,
    );
}

#[test]
fn live_target_mismatch_denies_and_inspection_failure_remains_error() {
    let (mut repository, mut filesystem, task) = fixture();
    declare(
        &mut repository,
        &mut filesystem,
        task.id(),
        task.version(),
        Vec::new(),
    );
    filesystem
        .identities
        .get_mut(Path::new("C:/managed/worktree"))
        .expect("worktree identity")
        .file_id_hex = "ffffffffffffffffffffffffffffffff".to_owned();
    assert_denied(
        evaluate(&mut repository, &mut filesystem, task.id(), task.version())
            .expect("identity mismatch is a decision"),
        PolicyDenialReason::TargetIdentityMismatch,
    );

    filesystem.failures.insert(
        PathBuf::from("C:/managed/worktree"),
        FailureCategory::StorageUnavailable,
    );
    let error = match evaluate(&mut repository, &mut filesystem, task.id(), task.version()) {
        Err(error) => error,
        Ok(_) => panic!("inspection failure must remain application error"),
    };
    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
}

#[test]
fn missing_approval_denies_but_corrupt_approval_remains_error() {
    let (mut repository, mut filesystem, task) = fixture();
    let category = HighRiskCategory::ArchitectureChange;
    repository.high_risk_approvals.insert(
        (task.id(), task.version(), category),
        HighRiskApprovalRecord {
            task_id: task.id(),
            approved_task_version: task.version(),
            risk_category: category,
            approved_at_ms: 190,
        },
    );
    declare(
        &mut repository,
        &mut filesystem,
        task.id(),
        task.version(),
        vec![category],
    );
    repository.high_risk_approvals.clear();
    assert_denied(
        evaluate(&mut repository, &mut filesystem, task.id(), task.version())
            .expect("missing approval is a decision"),
        PolicyDenialReason::ApprovalMissing,
    );

    repository.high_risk_approvals.insert(
        (task.id(), task.version(), category),
        HighRiskApprovalRecord {
            task_id: TaskId::new(),
            approved_task_version: task.version(),
            risk_category: category,
            approved_at_ms: 190,
        },
    );
    let error = match evaluate(&mut repository, &mut filesystem, task.id(), task.version()) {
        Err(error) => error,
        Ok(_) => panic!("corrupt approval must remain application error"),
    };
    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn repository_corruption_and_lookup_failure_remain_application_errors() {
    let (mut repository, mut filesystem, task) = fixture();
    declare(
        &mut repository,
        &mut filesystem,
        task.id(),
        task.version(),
        Vec::new(),
    );
    let key = (
        task.id(),
        task.version(),
        chatoms_domain::OperationRiskKind::ProviderImplementation,
    );
    repository
        .operation_risk_declarations
        .get_mut(&key)
        .expect("declaration fixture")
        .record
        .task_id = TaskId::new();
    let corruption = match evaluate(&mut repository, &mut filesystem, task.id(), task.version()) {
        Err(error) => error,
        Ok(_) => panic!("corrupt declaration must remain application error"),
    };
    assert_eq!(corruption.code(), ApplicationErrorCode::Internal);

    repository.fail_on = Some((
        "get_operation_risk_declaration",
        RepositoryErrorCode::OperationFailed,
    ));
    let failure = match evaluate(&mut repository, &mut filesystem, task.id(), task.version()) {
        Err(error) => error,
        Ok(_) => panic!("lookup failure must remain application error"),
    };
    assert_eq!(failure.code(), ApplicationErrorCode::Internal);
}
