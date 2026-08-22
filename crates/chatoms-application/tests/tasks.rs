mod support;

use std::str::FromStr;

use chatoms_application::{
    error::ApplicationErrorCode,
    tasks::{
        ApproveHighRiskOperationRequest, CreateTaskRequest,
        PrepareImplementationContextPackageRequest, PreparePlanningContextPackageRequest,
        PrepareReviewContextPackageRequest, RecordDiffApprovalRequest,
        RecordImplementationResultRequest, RecordPlanningResultRequest, RecordReviewResultRequest,
        StartContextPackageImplementationRequest, StartContextPackagePlanningRequest,
        StartImplementationRequest, StartPlanningRequest, StartReviewRequest, TaskActionRequest,
        TaskService, TransitionTaskRequest,
    },
};
use chatoms_domain::{ContextDataScope, HighRiskCategory, ProjectId, TaskId, TaskState, WorkKind};
use chatoms_ports::{
    diff::DiffContentHash,
    error::{CategorizedFailure, FailureCategory, PortFailure},
    provider::ProviderKind,
    repository::{
        ActiveLease, ContextPackageManifestRecord, GitIsolationStatus, ImplementationResultOutcome,
        PlanningResultOutcome, ProviderConsent, RepositoryErrorCode, ReviewResultOutcome,
        TaskBriefRecord, TaskGitIsolation, TaskPlanningResultRecord, TaskReviewResultRecord,
    },
};

use support::{FakeRepository, FakeTime, restored_task, storage_failure};

fn create_request() -> CreateTaskRequest {
    CreateTaskRequest::new(
        ProjectId::new(),
        "user".to_owned(),
        "task.created".to_owned(),
    )
}

fn action(task_id: TaskId, expected_version: u64) -> TaskActionRequest {
    TaskActionRequest::new(
        task_id,
        expected_version,
        "user".to_owned(),
        "task.action".to_owned(),
    )
}

fn transition(
    task_id: TaskId,
    expected_version: u64,
    target_state: TaskState,
) -> TransitionTaskRequest {
    TransitionTaskRequest::new(
        task_id,
        expected_version,
        target_state,
        "user".to_owned(),
        "task.transition".to_owned(),
    )
}

fn start_planning(task_id: TaskId, expected_version: u64) -> StartPlanningRequest {
    StartPlanningRequest::new(
        task_id,
        expected_version,
        "user".to_owned(),
        "task.planning.consent".to_owned(),
    )
}

fn start_implementation(task_id: TaskId, expected_version: u64) -> StartImplementationRequest {
    StartImplementationRequest::new(
        task_id,
        expected_version,
        "user".to_owned(),
        "task.implementation.consent".to_owned(),
    )
}

fn start_review(task_id: TaskId, expected_version: u64) -> StartReviewRequest {
    StartReviewRequest::new(task_id, expected_version)
}

fn prepare_planning(
    task_id: TaskId,
    expected_version: u64,
) -> PreparePlanningContextPackageRequest {
    PreparePlanningContextPackageRequest::new(task_id, expected_version)
}

fn prepare_implementation(
    task_id: TaskId,
    expected_version: u64,
) -> PrepareImplementationContextPackageRequest {
    PrepareImplementationContextPackageRequest::new(task_id, expected_version)
}

fn prepare_review(task_id: TaskId, expected_version: u64) -> PrepareReviewContextPackageRequest {
    PrepareReviewContextPackageRequest::new(task_id, expected_version)
}

fn start_context_package_planning(
    task_id: TaskId,
    expected_version: u64,
) -> StartContextPackagePlanningRequest {
    StartContextPackagePlanningRequest::new(
        task_id,
        expected_version,
        "user".to_owned(),
        "task.planning.context_package.transition".to_owned(),
    )
}

fn start_context_package_implementation(
    task_id: TaskId,
    expected_version: u64,
) -> StartContextPackageImplementationRequest {
    StartContextPackageImplementationRequest::new(
        task_id,
        expected_version,
        "user".to_owned(),
        "task.implementation.context_package.transition".to_owned(),
    )
}

fn worktree_ready_isolation(task_id: TaskId, expected_version: u64) -> TaskGitIsolation {
    TaskGitIsolation {
        task_id,
        project_id: ProjectId::new(),
        status: GitIsolationStatus::WorktreeReady,
        operation_id: Some(chatoms_domain::GitOperationId::new()),
        expected_task_version: expected_version,
        base_branch: Some("main".to_owned()),
        base_commit: Some("a".repeat(40)),
        worktree_path: Some("C:/managed/task".to_owned()),
        branch_created_by_app: true,
        worktree_created_by_app: true,
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

#[test]
fn create_builds_validated_task_initial_transition_and_atomic_repository_call() {
    let mut repository = FakeRepository::default();
    let mut time = FakeTime::at(1_000);
    let view = TaskService::new(&mut repository, &mut time)
        .create_task(create_request())
        .expect("create task");

    assert_eq!(view.state, TaskState::Created);
    assert_eq!(view.version, 0);
    assert_eq!(view.created_at_ms, 1_000);
    assert_eq!(view.updated_at_ms, 1_000);
    assert_eq!(view.terminal_at_ms, None);
    let canonical = view.id.to_string();
    assert_eq!(canonical.len(), 36);
    assert_eq!(canonical.as_bytes()[14], b'7');
    assert_eq!(
        TaskId::from_str(&canonical).expect("canonical UUIDv7"),
        view.id
    );
    assert_eq!(
        view.branch_identity.as_str(),
        format!("ai-task/{}", view.id)
    );
    assert_eq!(repository.calls, ["create_task"]);
    let (task, initial, lease_time) = repository.last_created.expect("created record");
    assert_eq!(task.id(), view.id);
    assert_eq!(lease_time, 1_000);
    assert_eq!(initial.sequence(), 1);
    assert_eq!(initial.from_state(), None);
    assert_eq!(initial.to_state(), TaskState::Created);
    assert_eq!(initial.task_version(), 0);
}

#[test]
fn create_validates_codes_and_time_before_repository_side_effects() {
    for request in [
        CreateTaskRequest::new(ProjectId::new(), "bad actor!".to_owned(), "ok".to_owned()),
        CreateTaskRequest::new(ProjectId::new(), "ok".to_owned(), "bad reason!".to_owned()),
    ] {
        let mut repository = FakeRepository::default();
        let mut time = FakeTime::at(10);
        let result = TaskService::new(&mut repository, &mut time).create_task(request);
        let error = result.expect_err("invalid code");
        assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
        assert!(repository.calls.is_empty());
    }

    let mut repository = FakeRepository::default();
    let mut time = FakeTime {
        now: 10,
        failure: Some(storage_failure()),
        calls: 0,
    };
    let error = TaskService::new(&mut repository, &mut time)
        .create_task(create_request())
        .expect_err("clock failure");
    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
    assert!(repository.calls.is_empty());
}

#[test]
fn create_repository_failures_return_no_partial_result_and_map_stably() {
    for (repository_code, expected) in [
        (
            RepositoryErrorCode::ActiveLeaseConflict,
            ApplicationErrorCode::ActiveTaskConflict,
        ),
        (
            RepositoryErrorCode::ProjectNotFound,
            ApplicationErrorCode::NotFound,
        ),
        (
            RepositoryErrorCode::DuplicateTask,
            ApplicationErrorCode::AlreadyExists,
        ),
    ] {
        let mut repository = FakeRepository {
            fail_on: Some(("create_task", repository_code)),
            ..FakeRepository::default()
        };
        let mut time = FakeTime::at(10);
        let error = TaskService::new(&mut repository, &mut time)
            .create_task(create_request())
            .expect_err("repository failure");
        assert_eq!(error.code(), expected);
        assert!(repository.last_created.is_none());
    }
}

#[test]
fn static_transition_updates_task_and_builds_repository_validated_sequence() {
    let (task, history) = restored_task(TaskState::Created, 0, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);
    let view = TaskService::new(&mut repository, &mut time)
        .transition_task(transition(task_id, 0, TaskState::ProjectValidated))
        .expect("transition");
    assert_eq!(view.state, TaskState::ProjectValidated);
    assert_eq!(view.version, 1);
    assert_eq!(view.updated_at_ms, 20);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 0);
    assert_eq!(saved.state(), TaskState::ProjectValidated);
    assert_eq!(record.sequence(), 2);
    assert_eq!(record.from_state(), Some(TaskState::Created));
    assert_eq!(record.to_state(), TaskState::ProjectValidated);
    assert_eq!(record.task_version(), 1);
    assert_eq!(
        repository.calls,
        ["get_task", "list_task_transitions", "save_transition"]
    );
}

#[test]
fn static_transition_rejects_invalid_context_version_time_and_missing_task() {
    let cases = [
        (
            1,
            TaskState::ProjectValidated,
            20,
            ApplicationErrorCode::VersionConflict,
        ),
        (0, TaskState::Paused, 20, ApplicationErrorCode::InvalidState),
        (
            0,
            TaskState::ProjectValidated,
            5,
            ApplicationErrorCode::InvalidInput,
        ),
    ];
    for (expected_version, target, now, expected_error) in cases {
        let (task, history) = restored_task(TaskState::Created, 0, 10, None);
        let task_id = task.id();
        let mut repository = FakeRepository::default();
        repository.seed_task(task, history);
        let mut time = FakeTime::at(now);
        let error = TaskService::new(&mut repository, &mut time)
            .transition_task(transition(task_id, expected_version, target))
            .expect_err("transition rejected");
        assert_eq!(error.code(), expected_error);
        assert!(repository.last_saved.is_none());
        assert_eq!(repository.tasks[&task_id].state(), TaskState::Created);
    }

    let mut repository = FakeRepository::default();
    let mut time = FakeTime::at(20);
    let error = TaskService::new(&mut repository, &mut time)
        .transition_task(transition(TaskId::new(), 0, TaskState::ProjectValidated))
        .expect_err("missing task");
    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
}

#[test]
fn sequence_and_repository_conflicts_map_without_mutating_stored_aggregate() {
    for (operation, repository_code, expected) in [
        (
            "list_task_transitions",
            RepositoryErrorCode::TransitionSequenceConflict,
            ApplicationErrorCode::SequenceConflict,
        ),
        (
            "save_transition",
            RepositoryErrorCode::VersionConflict,
            ApplicationErrorCode::VersionConflict,
        ),
    ] {
        let (task, history) = restored_task(TaskState::Created, 0, 10, None);
        let task_id = task.id();
        let mut repository = FakeRepository {
            fail_on: Some((operation, repository_code)),
            ..FakeRepository::default()
        };
        repository.seed_task(task, history);
        let mut time = FakeTime::at(20);
        let error = TaskService::new(&mut repository, &mut time)
            .transition_task(transition(task_id, 0, TaskState::ProjectValidated))
            .expect_err("repository conflict");
        assert_eq!(error.code(), expected);
        assert_eq!(repository.tasks[&task_id].state(), TaskState::Created);
        assert_eq!(repository.tasks[&task_id].version(), 0);
    }
}

#[test]
fn pause_and_recovery_entry_are_explicit_but_resume_validation_is_not_faked() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let paused = TaskService::new(&mut repository, &mut time)
        .pause_task(action(task_id, 1))
        .expect("pause");
    assert_eq!(paused.state, TaskState::Paused);
    assert_eq!(paused.resume_target_state, Some(TaskState::WorktreeReady));

    let (task, history) = restored_task(TaskState::WorktreeCreating, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let recovered = TaskService::new(&mut repository, &mut time)
        .mark_recovery_required(action(task_id, 1))
        .expect("mark recovery");
    assert_eq!(recovered.state, TaskState::RecoveryRequired);

    let calls_before = repository.calls.len();
    let mut service = TaskService::new(&mut repository, &mut time);
    for result in [
        service.resume_paused_task(task_id),
        service.set_recovery_target(task_id, TaskState::WorktreeReady),
        service.resume_recovered_task(task_id),
        service.pause_recovery_task(task_id),
    ] {
        assert_eq!(
            result.expect_err("validation capability is absent").code(),
            ApplicationErrorCode::Unsupported
        );
    }
    assert_eq!(repository.calls.len(), calls_before);
}

#[test]
fn reconcile_startup_planning_moves_a_leftover_planning_task_to_recovery_required_and_keeps_the_lease()
 {
    let (task, history) = restored_task(TaskState::Planning, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_planning()
        .expect("reconciliation succeeds")
        .expect("a leftover Planning task is reconciled");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 5);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 4);
    assert_eq!(saved.state(), TaskState::RecoveryRequired);
    assert_eq!(record.from_state(), Some(TaskState::Planning));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
    assert_eq!(
        repository.calls,
        [
            "active_lease",
            "get_task",
            "get_task",
            "list_task_transitions",
            "save_transition"
        ]
    );
}

#[test]
fn reconcile_startup_planning_is_a_no_op_without_an_active_task() {
    let mut repository = FakeRepository::default();
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_planning()
        .expect("no active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease"]);
}

#[test]
fn reconcile_startup_planning_is_a_no_op_when_the_active_task_is_not_planning() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_planning()
        .expect("a non-Planning active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease", "get_task"]);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn reconcile_startup_planning_fails_closed_on_repository_error_without_assuming_success() {
    let (task, history) = restored_task(TaskState::Planning, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("get_task", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_planning()
        .expect_err("a repository failure must surface, not be treated as reconciled");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Planning);
    assert_eq!(repository.tasks[&task_id].version(), 4);
    assert!(repository.last_saved.is_none());
}

#[test]
fn reconcile_startup_testing_moves_a_leftover_testing_task_to_recovery_required_and_keeps_the_lease()
 {
    let (task, history) = restored_task(TaskState::Testing, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_testing()
        .expect("reconciliation succeeds")
        .expect("a leftover Testing task is reconciled");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 5);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 4);
    assert_eq!(saved.state(), TaskState::RecoveryRequired);
    assert_eq!(record.from_state(), Some(TaskState::Testing));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
    assert_eq!(
        repository.calls,
        [
            "active_lease",
            "get_task",
            "get_task",
            "list_task_transitions",
            "save_transition"
        ]
    );
}

#[test]
fn reconcile_startup_testing_is_a_no_op_without_an_active_task() {
    let mut repository = FakeRepository::default();
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_testing()
        .expect("no active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease"]);
}

#[test]
fn reconcile_startup_testing_is_a_no_op_when_the_active_task_is_not_testing() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_testing()
        .expect("a non-Testing active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease", "get_task"]);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn reconcile_startup_testing_fails_closed_on_repository_error_without_assuming_success() {
    let (task, history) = restored_task(TaskState::Testing, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("get_task", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_testing()
        .expect_err("a repository failure must surface, not be treated as reconciled");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Testing);
    assert_eq!(repository.tasks[&task_id].version(), 4);
    assert!(repository.last_saved.is_none());
}

#[test]
fn reconcile_startup_reviewing_moves_a_leftover_reviewing_task_to_recovery_required_and_keeps_the_lease()
 {
    let (task, history) = restored_task(TaskState::Reviewing, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_reviewing()
        .expect("reconciliation succeeds")
        .expect("a leftover Reviewing task is reconciled");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 5);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 4);
    assert_eq!(saved.state(), TaskState::RecoveryRequired);
    assert_eq!(record.from_state(), Some(TaskState::Reviewing));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
    assert_eq!(
        repository.calls,
        [
            "active_lease",
            "get_task",
            "get_task",
            "list_task_transitions",
            "save_transition"
        ]
    );
}

#[test]
fn reconcile_startup_reviewing_is_a_no_op_without_an_active_task() {
    let mut repository = FakeRepository::default();
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_reviewing()
        .expect("no active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease"]);
}

#[test]
fn reconcile_startup_reviewing_is_a_no_op_when_the_active_task_is_not_reviewing() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_reviewing()
        .expect("a non-Reviewing active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease", "get_task"]);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn reconcile_startup_reviewing_fails_closed_on_repository_error_without_assuming_success() {
    let (task, history) = restored_task(TaskState::Reviewing, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("get_task", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_reviewing()
        .expect_err("a repository failure must surface, not be treated as reconciled");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
    assert_eq!(repository.tasks[&task_id].version(), 4);
    assert!(repository.last_saved.is_none());
}

#[test]
fn reconcile_startup_implementation_moves_a_leftover_implementing_task_to_recovery_required_and_keeps_the_lease()
 {
    let (task, history) = restored_task(TaskState::Implementing, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_implementation()
        .expect("reconciliation succeeds")
        .expect("a leftover Implementing task is reconciled");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 5);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 4);
    assert_eq!(saved.state(), TaskState::RecoveryRequired);
    assert_eq!(record.from_state(), Some(TaskState::Implementing));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
    assert_eq!(
        repository.calls,
        [
            "active_lease",
            "get_task",
            "get_task",
            "list_task_transitions",
            "save_transition"
        ]
    );
}

#[test]
fn reconcile_startup_implementation_is_a_no_op_without_an_active_task() {
    let mut repository = FakeRepository::default();
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_implementation()
        .expect("no active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease"]);
}

#[test]
fn reconcile_startup_implementation_is_a_no_op_when_the_active_task_is_not_implementing() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let result = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_implementation()
        .expect("a non-Implementing active task is not an error");

    assert!(result.is_none());
    assert_eq!(repository.calls, ["active_lease", "get_task"]);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn reconcile_startup_implementation_fails_closed_on_repository_error_without_assuming_success() {
    let (task, history) = restored_task(TaskState::Implementing, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("get_task", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .reconcile_startup_implementation()
        .expect_err("a repository failure must surface, not be treated as reconciled");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Implementing);
    assert_eq!(repository.tasks[&task_id].version(), 4);
    assert!(repository.last_saved.is_none());
}

#[test]
fn unknown_external_effect_and_general_api_cannot_bypass_contextual_rules() {
    let (task, history) = restored_task(TaskState::UnknownExternalEffect, 1, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let error = TaskService::new(&mut repository, &mut time)
        .transition_task(transition(task_id, 1, TaskState::WorktreeReady))
        .expect_err("direct resume forbidden");
    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
}

#[test]
fn complete_fail_cancel_use_atomic_terminal_repository_boundary() {
    for (state, terminal, operation) in [
        (
            TaskState::PostMergeTesting,
            TaskState::Completed,
            "complete",
        ),
        (TaskState::Created, TaskState::Failed, "fail"),
        (TaskState::Created, TaskState::Cancelled, "cancel"),
    ] {
        let version = usize::from(state != TaskState::Created) as u64;
        let (task, history) = restored_task(state, version, 20, None);
        let task_id = task.id();
        let mut repository = FakeRepository::default();
        repository.seed_task(task, history);
        let mut time = FakeTime::at(30);
        let mut service = TaskService::new(&mut repository, &mut time);
        let view = match operation {
            "complete" => service.complete_task(action(task_id, version)),
            "fail" => service.fail_task(action(task_id, version)),
            "cancel" => service.cancel_task(action(task_id, version)),
            _ => unreachable!(),
        }
        .expect("terminal transition");
        assert_eq!(view.state, terminal);
        assert_eq!(view.terminal_at_ms, Some(30));
        assert_eq!(view.resume_target_state, None);
        assert!(repository.last_terminated.is_some());
        assert!(repository.active_lease.is_none());
    }
}

#[test]
fn terminal_transition_clears_pause_target_and_rejects_terminal_or_bad_time() {
    let (task, history) = restored_task(TaskState::Paused, 1, 20, Some(TaskState::WorktreeReady));
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let view = TaskService::new(&mut repository, &mut time)
        .fail_task(action(task_id, 1))
        .expect("paused failure");
    assert_eq!(view.resume_target_state, None);

    for (state, now) in [(TaskState::Completed, 30), (TaskState::Created, 5)] {
        let version = usize::from(state != TaskState::Created) as u64;
        let (task, history) = restored_task(state, version, 20, None);
        let task_id = task.id();
        let mut repository = FakeRepository::default();
        repository.seed_task(task, history);
        let mut time = FakeTime::at(now);
        let error = TaskService::new(&mut repository, &mut time)
            .fail_task(action(task_id, version))
            .expect_err("terminal transition rejected");
        assert!(matches!(
            error.code(),
            ApplicationErrorCode::InvalidState | ApplicationErrorCode::InvalidInput
        ));
    }
}

#[test]
fn terminal_repository_failures_map_safely() {
    for (repository_code, expected) in [
        (
            RepositoryErrorCode::ActiveLeaseConflict,
            ApplicationErrorCode::ActiveTaskConflict,
        ),
        (
            RepositoryErrorCode::InvalidPersistenceState,
            ApplicationErrorCode::Internal,
        ),
    ] {
        let (task, history) = restored_task(TaskState::Created, 0, 10, None);
        let task_id = task.id();
        let mut repository = FakeRepository {
            fail_on: Some(("terminate_task", repository_code)),
            ..FakeRepository::default()
        };
        repository.seed_task(task, history);
        let mut time = FakeTime::at(20);
        let error = TaskService::new(&mut repository, &mut time)
            .fail_task(action(task_id, 0))
            .expect_err("terminal repository error");
        assert_eq!(error.code(), expected);
        for forbidden in ["C:\\private", "SELECT", "token", "S-1-5-"] {
            assert!(!error.to_string().contains(forbidden));
        }
    }
}

#[test]
fn active_task_task_and_history_are_read_only_application_views() {
    let (task, history) = restored_task(TaskState::Created, 0, 10, None);
    let task_id = task.id();
    let lease = ActiveLease {
        task_id,
        acquired_at_ms: 10,
    };
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository.active_lease = Some(lease);
    let mut time = FakeTime::at(20);
    let mut service = TaskService::new(&mut repository, &mut time);
    assert_eq!(
        service
            .get_active_task()
            .expect("active")
            .expect("lease")
            .task_id,
        task_id
    );
    assert_eq!(
        service
            .get_task(task_id)
            .expect("task")
            .expect("present")
            .state,
        TaskState::Created
    );
    let history = service.task_history(task_id).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].sequence, 1);
}

#[test]
fn get_task_includes_the_persisted_brief_when_present_and_none_otherwise() {
    let (task, history) = restored_task(TaskState::Created, 0, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let without_brief = TaskService::new(&mut repository, &mut time)
        .get_task(task_id)
        .expect("task")
        .expect("present");
    assert!(without_brief.brief.is_none());

    repository.briefs.insert(
        task_id,
        chatoms_ports::repository::TaskBriefRecord {
            task_id,
            requirements: "Add CSV export".to_owned(),
            completion_criteria: "Export button downloads a CSV".to_owned(),
            prohibited_scope: "Do not touch the import pipeline".to_owned(),
            created_at_ms: 10,
        },
    );
    let with_brief = TaskService::new(&mut repository, &mut time)
        .get_task(task_id)
        .expect("task")
        .expect("present")
        .brief
        .expect("brief present");
    assert_eq!(with_brief.requirements, "Add CSV export");
    assert_eq!(
        with_brief.completion_criteria,
        "Export button downloads a CSV"
    );
    assert_eq!(
        with_brief.prohibited_scope,
        "Do not touch the import pipeline"
    );
    assert_eq!(with_brief.created_at_ms, 10);
}

#[test]
fn time_provider_policy_preserves_category_severity_and_retry() {
    let mut repository = FakeRepository::default();
    let failure = PortFailure::with_policy(
        FailureCategory::Internal,
        chatoms_ports::error::FailureSeverity::Critical,
        chatoms_ports::error::RetryDisposition::Never,
    );
    let mut time = FakeTime {
        now: 0,
        failure: Some(failure),
        calls: 0,
    };
    let error = TaskService::new(&mut repository, &mut time)
        .create_task(create_request())
        .expect_err("time error");
    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(error.severity(), failure.severity());
    assert_eq!(error.retry(), failure.retry());
}

#[test]
fn start_planning_records_new_consent_and_transitions_atomically() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 1))
        .expect("start planning");

    assert_eq!(view.state, TaskState::Planning);
    assert_eq!(view.version, 2);
    assert_eq!(view.updated_at_ms, 20);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 1);
    assert_eq!(saved.state(), TaskState::Planning);
    assert_eq!(record.from_state(), Some(TaskState::WorktreeReady));
    assert_eq!(record.to_state(), TaskState::Planning);
    let consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("consent recorded");
    assert_eq!(consent.consented_at_ms, 20);
    assert!(repository.calls.contains(&"save_planning_transition"));
}

#[test]
fn start_planning_reuses_an_existing_same_version_consent_without_overwriting_it() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let existing_consent = chatoms_ports::repository::ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 5,
    };
    repository.consents.insert(
        (
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        ),
        existing_consent,
    );
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 1))
        .expect("start planning reuses consent");

    assert_eq!(view.state, TaskState::Planning);
    let consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("consent still present");
    assert_eq!(
        consent.consented_at_ms, 5,
        "reused consent must not be overwritten with a new timestamp"
    );
    assert_eq!(repository.consents.len(), 1);
}

#[test]
fn start_planning_rejects_when_task_is_not_worktree_ready() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 2))
        .expect_err("wrong state must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
    assert!(repository.consents.is_empty());
}

#[test]
fn start_planning_rejects_when_isolation_is_not_worktree_ready() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut not_ready = worktree_ready_isolation(task_id, 1);
    not_ready.status = GitIsolationStatus::RecoveryRequired;
    repository.isolations.insert(task_id, not_ready);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 1))
        .expect_err("unready isolation must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
    assert!(repository.consents.is_empty());
}

#[test]
fn start_planning_rejects_missing_isolation_record() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 1))
        .expect_err("missing isolation must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert!(repository.last_saved.is_none());
}

#[test]
fn start_planning_version_mismatch_leaves_no_partial_write() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 99))
        .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.last_saved.is_none());
    assert!(repository.consents.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn start_planning_repository_failure_does_not_record_consent_or_advance_task() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "save_planning_transition",
            RepositoryErrorCode::OperationFailed,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 1))
        .expect_err("repository failure must surface");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(repository.consents.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
    assert_eq!(repository.tasks[&task_id].version(), 1);
}

#[test]
fn start_implementation_records_new_consent_and_transitions_atomically() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_implementation(start_implementation(task_id, 1))
        .expect("start implementation");

    assert_eq!(view.state, TaskState::Implementing);
    assert_eq!(view.version, 2);
    assert_eq!(view.updated_at_ms, 20);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 1);
    assert_eq!(saved.state(), TaskState::Implementing);
    assert_eq!(record.from_state(), Some(TaskState::AwaitingDesignApproval));
    assert_eq!(record.to_state(), TaskState::Implementing);
    let consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("consent recorded");
    assert_eq!(consent.consented_at_ms, 20);
    assert!(repository.calls.contains(&"save_implementation_transition"));
}

#[test]
fn start_implementation_reuses_an_existing_same_version_consent_without_overwriting_it() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let existing_consent = chatoms_ports::repository::ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 5,
    };
    repository.consents.insert(
        (
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            1,
            ContextDataScope::LegacyPhase4,
        ),
        existing_consent,
    );
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_implementation(start_implementation(task_id, 1))
        .expect("start implementation reuses consent");

    assert_eq!(view.state, TaskState::Implementing);
    let consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("consent still present");
    assert_eq!(
        consent.consented_at_ms, 5,
        "reused consent must not be overwritten with a new timestamp"
    );
    assert_eq!(repository.consents.len(), 1);
}

#[test]
fn start_implementation_rejects_when_task_is_not_awaiting_design_approval() {
    let (task, history) = restored_task(TaskState::Implementing, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_implementation(start_implementation(task_id, 2))
        .expect_err("wrong state must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
    assert!(repository.consents.is_empty());
}

#[test]
fn start_implementation_version_mismatch_leaves_no_partial_write() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_implementation(start_implementation(task_id, 99))
        .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.last_saved.is_none());
    assert!(repository.consents.is_empty());
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
}

#[test]
fn start_implementation_repository_failure_does_not_record_consent_or_advance_task() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "save_implementation_transition",
            RepositoryErrorCode::OperationFailed,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_implementation(start_implementation(task_id, 1))
        .expect_err("repository failure must surface");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(repository.consents.is_empty());
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert_eq!(repository.tasks[&task_id].version(), 1);
}

#[test]
fn start_implementation_consent_is_independent_of_a_planning_consent_for_the_same_task_and_version()
{
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let planning_consent = chatoms_ports::repository::ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 5,
    };
    repository.consents.insert(
        (
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        ),
        planning_consent,
    );
    let mut time = FakeTime::at(20);

    TaskService::new(&mut repository, &mut time)
        .start_implementation(start_implementation(task_id, 1))
        .expect("a Planning consent must not be treated as an Implementation consent");

    let implementation_consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("a fresh Implementation consent must have been recorded");
    assert_eq!(implementation_consent.consented_at_ms, 20);
    assert_eq!(
        repository
            .consents
            .get(&(
                task_id,
                ProviderKind::Claude,
                WorkKind::Planning,
                1,
                ContextDataScope::LegacyPhase4
            ))
            .expect("the pre-existing Planning consent must be untouched"),
        &planning_consent
    );
    assert_eq!(repository.consents.len(), 2);
}

#[test]
fn start_review_records_new_consent_without_transitioning_or_writing_history() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 1))
        .expect("start review");

    assert_eq!(view.state, TaskState::Reviewing);
    assert_eq!(view.version, 1);
    let consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("consent recorded");
    assert_eq!(consent.consented_at_ms, 20);
    assert!(repository.calls.contains(&"save_review_consent"));
    assert!(
        repository.last_saved.is_none(),
        "start_review must never drive a state transition, unlike start_planning/start_implementation"
    );
    assert_eq!(repository.transitions[&task_id].len(), 1);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
    assert_eq!(repository.tasks[&task_id].version(), 1);
}

#[test]
fn start_review_reuses_an_existing_same_version_consent_without_overwriting_it() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let existing_consent = chatoms_ports::repository::ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 5,
    };
    repository.consents.insert(
        (
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            1,
            ContextDataScope::LegacyPhase4,
        ),
        existing_consent,
    );
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 1))
        .expect("start review reuses consent");

    assert_eq!(view.state, TaskState::Reviewing);
    let consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("consent still present");
    assert_eq!(
        consent.consented_at_ms, 5,
        "reused consent must not be overwritten with a new timestamp"
    );
    assert_eq!(repository.consents.len(), 1);
}

#[test]
fn start_review_rejects_when_task_is_not_reviewing() {
    let (task, history) = restored_task(TaskState::Testing, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 2))
        .expect_err("wrong state must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.consents.is_empty());
}

#[test]
fn start_review_version_mismatch_leaves_no_partial_write() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 99))
        .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.consents.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
    assert_eq!(repository.tasks[&task_id].version(), 1);
}

#[test]
fn start_review_repository_failure_does_not_record_consent() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("save_review_consent", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 1))
        .expect_err("repository failure must surface");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(repository.consents.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
    assert_eq!(repository.tasks[&task_id].version(), 1);
}

#[test]
fn start_review_consent_is_independent_of_planning_and_implementation_consents_for_the_same_task_and_version()
 {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let planning_consent = chatoms_ports::repository::ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 5,
    };
    let implementation_consent = chatoms_ports::repository::ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 6,
    };
    repository.consents.insert(
        (
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        ),
        planning_consent,
    );
    repository.consents.insert(
        (
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            1,
            ContextDataScope::LegacyPhase4,
        ),
        implementation_consent,
    );
    let mut time = FakeTime::at(20);

    TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 1))
        .expect("a Planning or Implementation consent must not be treated as a Review consent");

    let review_consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            1,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("a fresh Review consent must have been recorded");
    assert_eq!(review_consent.consented_at_ms, 20);
    assert_eq!(
        repository
            .consents
            .get(&(
                task_id,
                ProviderKind::Claude,
                WorkKind::Planning,
                1,
                ContextDataScope::LegacyPhase4
            ))
            .expect("the pre-existing Planning consent must be untouched"),
        &planning_consent
    );
    assert_eq!(
        repository
            .consents
            .get(&(
                task_id,
                ProviderKind::Claude,
                WorkKind::Implementation,
                1,
                ContextDataScope::LegacyPhase4
            ))
            .expect("the pre-existing Implementation consent must be untouched"),
        &implementation_consent
    );
    assert_eq!(repository.consents.len(), 3);
}

fn record_result(
    task_id: TaskId,
    expected_version: u64,
    outcome: PlanningResultOutcome,
    exit_code: Option<i32>,
    turn_count: Option<u32>,
    plan_text: Option<String>,
) -> RecordPlanningResultRequest {
    RecordPlanningResultRequest::new(
        task_id,
        expected_version,
        outcome,
        exit_code,
        turn_count,
        plan_text,
        5,
        "provider".to_owned(),
        "task.planning.result".to_owned(),
    )
}

#[test]
fn record_planning_result_success_reaches_awaiting_design_approval_atomically() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::Completed,
            Some(0),
            Some(5),
            Some("masked plan text".to_owned()),
        ))
        .expect("record success result");

    assert_eq!(view.state, TaskState::AwaitingDesignApproval);
    assert_eq!(view.version, 3);
    assert_eq!(view.updated_at_ms, 30);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 2);
    assert_eq!(saved.state(), TaskState::AwaitingDesignApproval);
    assert_eq!(record.from_state(), Some(TaskState::Planning));
    assert_eq!(record.to_state(), TaskState::AwaitingDesignApproval);
    let stored = repository
        .planning_results
        .get(&task_id)
        .expect("planning result stored");
    assert_eq!(stored.outcome, PlanningResultOutcome::Completed);
    assert_eq!(stored.exit_code, Some(0));
    assert_eq!(stored.turn_count, Some(5));
    assert_eq!(stored.plan_text.as_deref(), Some("masked plan text"));
    assert_eq!(stored.started_at_ms, 5);
    assert_eq!(stored.completed_at_ms, 30);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "AwaitingDesignApproval still requires the active lease"
    );
}

#[test]
fn get_planning_result_returns_the_stored_safe_record() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 30, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository.planning_results.insert(
        task_id,
        TaskPlanningResultRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            outcome: PlanningResultOutcome::Completed,
            exit_code: Some(0),
            turn_count: Some(4),
            started_at_ms: 5,
            completed_at_ms: 30,
            plan_text: Some("masked plan text".to_owned()),
        },
    );
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .get_planning_result(task_id)
        .expect("planning result lookup succeeds")
        .expect("a result is recorded");

    assert_eq!(view.outcome, PlanningResultOutcome::Completed);
    assert_eq!(view.plan_text.as_deref(), Some("masked plan text"));
    assert_eq!(view.turn_count, Some(4));
}

#[test]
fn get_planning_result_reports_none_when_no_attempt_has_been_recorded() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 30, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .get_planning_result(task_id)
        .expect("planning result lookup succeeds");

    assert!(view.is_none());
}

#[test]
fn get_review_result_returns_the_stored_safe_record() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 3, 30, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository.review_results.insert(
        task_id,
        TaskReviewResultRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            outcome: ReviewResultOutcome::Completed,
            exit_code: Some(0),
            turn_count: Some(4),
            started_at_ms: 5,
            completed_at_ms: 30,
            review_text: Some("masked review text".to_owned()),
        },
    );
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .get_review_result(task_id)
        .expect("review result lookup succeeds")
        .expect("a result is recorded");

    assert_eq!(view.outcome, ReviewResultOutcome::Completed);
    assert_eq!(view.review_text.as_deref(), Some("masked review text"));
    assert_eq!(view.turn_count, Some(4));
}

#[test]
fn get_review_result_reports_none_when_no_attempt_has_been_recorded() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 3, 30, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .get_review_result(task_id)
        .expect("review result lookup succeeds");

    assert!(view.is_none());
}

#[test]
fn record_planning_result_failed_transitions_to_failed_and_releases_the_lease() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::Failed,
            Some(1),
            None,
            None,
        ))
        .expect("record failed result");

    assert_eq!(view.state, TaskState::Failed);
    let stored = repository
        .planning_results
        .get(&task_id)
        .expect("planning result stored");
    assert_eq!(stored.outcome, PlanningResultOutcome::Failed);
    assert_eq!(stored.plan_text, None);
    assert!(
        repository.active_lease.is_none(),
        "a terminal outcome must release the active lease"
    );
}

#[test]
fn record_planning_result_recovery_required_keeps_the_lease() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::RecoveryRequired,
            Some(0),
            None,
            None,
        ))
        .expect("record recovery-required result");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(
        repository
            .planning_results
            .get(&task_id)
            .expect("planning result stored")
            .outcome,
        PlanningResultOutcome::RecoveryRequired
    );
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn record_planning_result_cancelled_transitions_to_cancelled_and_releases_the_lease() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::Cancelled,
            None,
            None,
            None,
        ))
        .expect("record a confirmed cancellation");

    assert_eq!(view.state, TaskState::Cancelled);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 2);
    assert_eq!(saved.state(), TaskState::Cancelled);
    assert_eq!(record.from_state(), Some(TaskState::Planning));
    assert_eq!(record.to_state(), TaskState::Cancelled);
    let stored = repository
        .planning_results
        .get(&task_id)
        .expect("planning result stored");
    assert_eq!(stored.outcome, PlanningResultOutcome::Cancelled);
    assert_eq!(stored.plan_text, None);
    assert!(
        repository.active_lease.is_none(),
        "a confirmed cancellation must release the active lease"
    );
}

#[test]
fn record_planning_result_rejects_plan_text_present_on_a_non_completed_outcome() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::Failed,
            Some(1),
            None,
            Some("should never be attached to a Failed outcome".to_owned()),
        ))
        .expect_err("plan text must not accompany a non-Completed outcome");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_planning_result_rejects_missing_plan_text_on_a_completed_outcome() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::Completed,
            Some(0),
            Some(1),
            None,
        ))
        .expect_err("a Completed outcome must carry non-empty plan text");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_planning_result_rejects_when_task_is_not_in_planning() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            1,
            PlanningResultOutcome::Completed,
            Some(0),
            Some(1),
            Some("plan".to_owned()),
        ))
        .expect_err("wrong state must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_planning_result_version_mismatch_leaves_no_partial_write() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let error = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            99,
            PlanningResultOutcome::Completed,
            Some(0),
            Some(1),
            Some("plan".to_owned()),
        ))
        .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.last_saved.is_none());
    assert!(repository.planning_results.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Planning);
}

#[test]
fn record_planning_result_persistence_failure_falls_back_to_recovery_required_without_a_result_row()
{
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("save_planning_result", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);

    let view = TaskService::new(&mut repository, &mut time)
        .record_planning_result(record_result(
            task_id,
            2,
            PlanningResultOutcome::Completed,
            Some(0),
            Some(5),
            Some("plan".to_owned()),
        ))
        .expect("falls back to RecoveryRequired instead of surfacing the raw failure");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 3);
    assert!(
        repository.planning_results.is_empty(),
        "the failed primary write must leave no planning result row behind"
    );
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
}

fn record_implementation_result(
    task_id: TaskId,
    expected_version: u64,
    outcome: ImplementationResultOutcome,
    exit_code: Option<i32>,
    turn_count: Option<u32>,
) -> RecordImplementationResultRequest {
    RecordImplementationResultRequest::new(
        task_id,
        expected_version,
        outcome,
        exit_code,
        turn_count,
        155,
        "provider".to_owned(),
        "task.implementation.result".to_owned(),
    )
}

#[test]
fn record_implementation_result_completed_transitions_to_testing_and_keeps_the_lease() {
    let (task, history) = restored_task(TaskState::Implementing, 4, 160, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(170);

    let view = TaskService::new(&mut repository, &mut time)
        .record_implementation_result(record_implementation_result(
            task_id,
            4,
            ImplementationResultOutcome::Completed,
            Some(0),
            Some(6),
        ))
        .expect("record completed result");

    assert_eq!(view.state, TaskState::Testing);
    assert_eq!(view.version, 5);
    assert_eq!(view.updated_at_ms, 170);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 4);
    assert_eq!(saved.state(), TaskState::Testing);
    assert_eq!(record.from_state(), Some(TaskState::Implementing));
    assert_eq!(record.to_state(), TaskState::Testing);
    let stored = repository
        .implementation_results
        .get(&task_id)
        .expect("implementation result stored");
    assert_eq!(stored.outcome, ImplementationResultOutcome::Completed);
    assert_eq!(stored.exit_code, Some(0));
    assert_eq!(stored.turn_count, Some(6));
    assert_eq!(stored.started_at_ms, 155);
    assert_eq!(stored.completed_at_ms, 170);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "Testing still requires the active lease"
    );
}

#[test]
fn record_implementation_result_confirmed_cancellation_pauses_with_implementing_resume_target_and_keeps_the_lease()
 {
    let (task, history) = restored_task(TaskState::Implementing, 4, 160, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(170);

    let view = TaskService::new(&mut repository, &mut time)
        .record_implementation_result(record_implementation_result(
            task_id,
            4,
            ImplementationResultOutcome::Cancelled,
            None,
            None,
        ))
        .expect("record a confirmed cancellation");

    assert_eq!(view.state, TaskState::Paused);
    assert_eq!(
        view.resume_target_state,
        Some(TaskState::Implementing),
        "a confirmed cancellation must be resumable back to Implementing"
    );
    let stored = repository
        .implementation_results
        .get(&task_id)
        .expect("implementation result stored");
    assert_eq!(stored.outcome, ImplementationResultOutcome::Cancelled);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "Paused still requires the active lease"
    );
}

#[test]
fn record_implementation_result_recovery_required_keeps_the_lease() {
    let (task, history) = restored_task(TaskState::Implementing, 4, 160, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(170);

    let view = TaskService::new(&mut repository, &mut time)
        .record_implementation_result(record_implementation_result(
            task_id,
            4,
            ImplementationResultOutcome::RecoveryRequired,
            Some(1),
            None,
        ))
        .expect("record recovery-required result");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(
        repository
            .implementation_results
            .get(&task_id)
            .expect("implementation result stored")
            .outcome,
        ImplementationResultOutcome::RecoveryRequired
    );
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn record_implementation_result_rejects_when_task_is_not_implementing() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 150, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(170);

    let error = TaskService::new(&mut repository, &mut time)
        .record_implementation_result(record_implementation_result(
            task_id,
            3,
            ImplementationResultOutcome::Completed,
            Some(0),
            Some(1),
        ))
        .expect_err("wrong state must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_implementation_result_version_mismatch_leaves_no_partial_write() {
    let (task, history) = restored_task(TaskState::Implementing, 4, 160, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(170);

    let error = TaskService::new(&mut repository, &mut time)
        .record_implementation_result(record_implementation_result(
            task_id,
            99,
            ImplementationResultOutcome::Completed,
            Some(0),
            Some(1),
        ))
        .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.last_saved.is_none());
    assert!(repository.implementation_results.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Implementing);
}

#[test]
fn record_implementation_result_persistence_failure_falls_back_to_recovery_required_without_a_result_row()
 {
    let (task, history) = restored_task(TaskState::Implementing, 4, 160, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "save_implementation_result",
            RepositoryErrorCode::OperationFailed,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(170);

    let view = TaskService::new(&mut repository, &mut time)
        .record_implementation_result(record_implementation_result(
            task_id,
            4,
            ImplementationResultOutcome::Completed,
            Some(0),
            Some(6),
        ))
        .expect("falls back to RecoveryRequired instead of surfacing the raw failure");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 5);
    assert!(
        repository.implementation_results.is_empty(),
        "the failed primary write must leave no implementation result row behind"
    );
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
}

fn record_review_result(
    task_id: TaskId,
    expected_version: u64,
    outcome: ReviewResultOutcome,
    exit_code: Option<i32>,
    turn_count: Option<u32>,
    review_text: Option<String>,
) -> RecordReviewResultRequest {
    RecordReviewResultRequest::new(
        task_id,
        expected_version,
        outcome,
        exit_code,
        turn_count,
        review_text,
        175,
        "provider".to_owned(),
        "task.review.result".to_owned(),
    )
}

#[test]
fn record_review_result_completed_reaches_awaiting_user_diff_approval_and_keeps_the_lease() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let view = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Completed,
            Some(0),
            Some(4),
            Some("masked review text".to_owned()),
        ))
        .expect("record completed result");

    assert_eq!(view.state, TaskState::AwaitingUserDiffApproval);
    assert_eq!(view.version, 7);
    assert_eq!(view.updated_at_ms, 190);
    let (expected_version, saved, record) = repository.last_saved.expect("saved transition");
    assert_eq!(expected_version, 6);
    assert_eq!(saved.state(), TaskState::AwaitingUserDiffApproval);
    assert_eq!(record.from_state(), Some(TaskState::Reviewing));
    assert_eq!(record.to_state(), TaskState::AwaitingUserDiffApproval);
    let stored = repository
        .review_results
        .get(&task_id)
        .expect("review result stored");
    assert_eq!(stored.outcome, ReviewResultOutcome::Completed);
    assert_eq!(stored.exit_code, Some(0));
    assert_eq!(stored.turn_count, Some(4));
    assert_eq!(stored.review_text.as_deref(), Some("masked review text"));
    assert_eq!(stored.started_at_ms, 175);
    assert_eq!(stored.completed_at_ms, 190);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "AwaitingUserDiffApproval still requires the active lease"
    );
}

#[test]
fn record_review_result_failed_transitions_to_failed_and_releases_the_lease() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let view = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Failed,
            Some(1),
            None,
            None,
        ))
        .expect("record failed result");

    assert_eq!(view.state, TaskState::Failed);
    let stored = repository
        .review_results
        .get(&task_id)
        .expect("review result stored");
    assert_eq!(stored.outcome, ReviewResultOutcome::Failed);
    assert_eq!(stored.review_text, None);
    assert!(
        repository.active_lease.is_none(),
        "a terminal Failed outcome must release the active lease"
    );
}

#[test]
fn record_review_result_confirmed_cancellation_pauses_with_reviewing_resume_target_and_keeps_the_lease()
 {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let view = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Cancelled,
            None,
            None,
            None,
        ))
        .expect("record a confirmed cancellation");

    assert_eq!(view.state, TaskState::Paused);
    assert_eq!(
        view.resume_target_state,
        Some(TaskState::Reviewing),
        "a confirmed cancellation must be resumable back to Reviewing"
    );
    let stored = repository
        .review_results
        .get(&task_id)
        .expect("review result stored");
    assert_eq!(stored.outcome, ReviewResultOutcome::Cancelled);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "Paused still requires the active lease"
    );
}

#[test]
fn record_review_result_recovery_required_keeps_the_lease() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let view = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::RecoveryRequired,
            Some(1),
            None,
            None,
        ))
        .expect("record recovery-required result");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(
        repository
            .review_results
            .get(&task_id)
            .expect("review result stored")
            .outcome,
        ReviewResultOutcome::RecoveryRequired
    );
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn record_review_result_rejects_review_text_present_on_a_non_completed_outcome() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let error = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Failed,
            Some(1),
            None,
            Some("should never be attached to a Failed outcome".to_owned()),
        ))
        .expect_err("review text must not accompany a non-Completed outcome");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_review_result_rejects_missing_review_text_on_a_completed_outcome() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let error = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Completed,
            Some(0),
            Some(1),
            None,
        ))
        .expect_err("a Completed outcome must carry non-empty review text");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_review_result_rejects_when_task_is_not_reviewing() {
    let (task, history) = restored_task(TaskState::Testing, 5, 170, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let error = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            5,
            ReviewResultOutcome::Completed,
            Some(0),
            Some(1),
            Some("review".to_owned()),
        ))
        .expect_err("wrong state must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.last_saved.is_none());
}

#[test]
fn record_review_result_version_mismatch_leaves_no_partial_write() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let error = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            99,
            ReviewResultOutcome::Completed,
            Some(0),
            Some(1),
            Some("review".to_owned()),
        ))
        .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.last_saved.is_none());
    assert!(repository.review_results.is_empty());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
}

#[test]
fn record_review_result_persistence_failure_falls_back_to_recovery_required_without_a_result_row() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("save_review_result", RepositoryErrorCode::OperationFailed)),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let view = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Completed,
            Some(0),
            Some(4),
            Some("review".to_owned()),
        ))
        .expect("falls back to RecoveryRequired instead of surfacing the raw failure");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(view.version, 7);
    assert!(
        repository.review_results.is_empty(),
        "the failed primary write must leave no review result row behind"
    );
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn record_review_result_persistence_failure_fallback_also_fails_and_propagates_the_original_error()
{
    let (task, history) = restored_task(TaskState::Reviewing, 6, 180, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some(("save_review_result", RepositoryErrorCode::OperationFailed)),
        fail_save_transition_once: Some(RepositoryErrorCode::OperationFailed),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(190);

    let error = TaskService::new(&mut repository, &mut time)
        .record_review_result(record_review_result(
            task_id,
            6,
            ReviewResultOutcome::Completed,
            Some(0),
            Some(4),
            Some("review".to_owned()),
        ))
        .expect_err(
            "when the RecoveryRequired fallback's own write also fails, the original error \
             must propagate, not a false success",
        );

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(
        repository.review_results.is_empty(),
        "no review result row must be written when both the primary write and its fallback fail"
    );
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::Reviewing,
        "the task must be left exactly as it was, not silently advanced, when recovery itself fails"
    );
}

#[test]
fn prepare_planning_context_package_calls_the_repository_and_returns_the_preparation() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let mut time = FakeTime::at(20);

    let preparation = TaskService::new(&mut repository, &mut time)
        .prepare_planning_context_package(prepare_planning(task_id, 1))
        .expect("prepare planning context package");

    assert_eq!(preparation.consent.task_id, task_id);
    assert_eq!(preparation.consent.work_kind, WorkKind::Planning);
    assert_eq!(
        preparation.consent.data_scope,
        ContextDataScope::ContextPackageV1
    );
    assert_eq!(preparation.consent.approved_task_version, 1);
    assert_eq!(preparation.manifest.work_kind, WorkKind::Planning);
    assert!(
        repository
            .calls
            .contains(&"prepare_planning_context_package")
    );
    assert!(
        repository.last_saved.is_none(),
        "preparation must never drive a state transition"
    );
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::WorktreeReady,
        "preparation must leave the task's state exactly as it was"
    );
}

#[test]
fn prepare_planning_context_package_rejects_wrong_state_without_calling_the_repository() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .prepare_planning_context_package(prepare_planning(task_id, 2))
        .expect_err("Planning must not be accepted as WorktreeReady");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"prepare_planning_context_package"),
        "a precondition failure must short-circuit before ever calling the repository"
    );
}

#[test]
fn prepare_planning_context_package_propagates_a_repository_failure_without_converting_it_to_success()
 {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "prepare_planning_context_package",
            RepositoryErrorCode::InvalidPersistenceState,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .prepare_planning_context_package(prepare_planning(task_id, 1))
        .expect_err("a repository failure must propagate as an error, never Ok or None");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn prepare_implementation_context_package_calls_the_repository_and_returns_the_preparation() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let preparation = TaskService::new(&mut repository, &mut time)
        .prepare_implementation_context_package(prepare_implementation(task_id, 3))
        .expect("prepare implementation context package");

    assert_eq!(preparation.consent.work_kind, WorkKind::Implementation);
    assert_eq!(
        preparation.consent.data_scope,
        ContextDataScope::ContextPackageV1
    );
    assert!(
        repository
            .calls
            .contains(&"prepare_implementation_context_package")
    );
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
}

#[test]
fn prepare_implementation_context_package_rejects_wrong_state_without_calling_the_repository() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .prepare_implementation_context_package(prepare_implementation(task_id, 1))
        .expect_err("WorktreeReady must not be accepted as AwaitingDesignApproval");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"prepare_implementation_context_package")
    );
}

#[test]
fn prepare_implementation_context_package_propagates_a_repository_failure() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "prepare_implementation_context_package",
            RepositoryErrorCode::InvalidPersistenceState,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .prepare_implementation_context_package(prepare_implementation(task_id, 3))
        .expect_err("a repository failure must propagate as an error, never Ok or None");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn prepare_review_context_package_calls_the_repository_and_returns_the_preparation() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let preparation = TaskService::new(&mut repository, &mut time)
        .prepare_review_context_package(prepare_review(task_id, 6))
        .expect("prepare review context package");

    assert_eq!(preparation.consent.work_kind, WorkKind::Review);
    assert_eq!(
        preparation.consent.data_scope,
        ContextDataScope::ContextPackageV1
    );
    assert!(repository.calls.contains(&"prepare_review_context_package"));
}

#[test]
fn prepare_review_context_package_rejects_wrong_state_without_calling_the_repository() {
    let (task, history) = restored_task(TaskState::Testing, 5, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .prepare_review_context_package(prepare_review(task_id, 5))
        .expect_err("Testing must not be accepted as Reviewing");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(!repository.calls.contains(&"prepare_review_context_package"));
}

#[test]
fn prepare_review_context_package_propagates_a_repository_failure() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "prepare_review_context_package",
            RepositoryErrorCode::InvalidPersistenceState,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .prepare_review_context_package(prepare_review(task_id, 6))
        .expect_err("a repository failure must propagate as an error, never Ok or None");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn context_package_v1_preparation_coexists_with_an_unmodified_legacy_phase4_start_review() {
    let (task, history) = restored_task(TaskState::Reviewing, 6, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let legacy_view = TaskService::new(&mut repository, &mut time)
        .start_review(start_review(task_id, 6))
        .expect("existing LegacyPhase4 start_review must be entirely unaffected by this Unit");
    assert_eq!(legacy_view.state, TaskState::Reviewing);

    let preparation = TaskService::new(&mut repository, &mut time)
        .prepare_review_context_package(prepare_review(task_id, 6))
        .expect(
            "ContextPackageV1 preparation must succeed independently of the LegacyPhase4 consent",
        );

    let legacy_consent = repository
        .consents
        .get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            6,
            ContextDataScope::LegacyPhase4,
        ))
        .expect("the LegacyPhase4 consent recorded by start_review must still be present");
    assert_ne!(
        preparation.consent, *legacy_consent,
        "the two scopes must be recorded as independent consents"
    );
    assert_eq!(repository.consents.len(), 2, "both scopes coexist");
}

fn seed_context_package_planning_pair(
    repository: &mut FakeRepository,
    task_id: TaskId,
    expected_version: u64,
    at_ms: i64,
) {
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Planning,
        expected_version,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: at_ms,
        },
    );
    repository.context_package_manifests.insert(
        key,
        ContextPackageManifestRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            created_at_ms: at_ms,
        },
    );
}

#[test]
fn context_package_planning_readiness_is_true_only_when_both_consent_and_manifest_exist() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_context_package_planning_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(20);

    let readiness = TaskService::new(&mut repository, &mut time)
        .get_context_package_planning_readiness(task_id, 1)
        .expect("readiness lookup");

    assert!(readiness.ready);
}

#[test]
fn context_package_planning_readiness_is_false_when_neither_consent_nor_manifest_exist() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let readiness = TaskService::new(&mut repository, &mut time)
        .get_context_package_planning_readiness(task_id, 1)
        .expect("readiness lookup");

    assert!(!readiness.ready);
}

#[test]
fn context_package_planning_readiness_is_a_fail_closed_error_on_a_partial_pair() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Planning,
        1,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: 1,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: 200,
        },
    );
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_context_package_planning_readiness(task_id, 1)
        .expect_err("a consent-only partial pair must never be reported as ready: false");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn context_package_planning_readiness_propagates_a_repository_failure_without_converting_it_to_ready_false()
 {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "get_provider_consent",
            RepositoryErrorCode::DatabaseUnavailable,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_context_package_planning_readiness(task_id, 1)
        .expect_err("a repository failure must propagate, never become ready: false");

    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
}

#[test]
fn start_context_package_planning_commits_the_transition_when_the_pair_is_prepared() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    seed_context_package_planning_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let view = TaskService::new(&mut repository, &mut time)
        .start_context_package_planning(start_context_package_planning(task_id, 1))
        .expect("start context package planning");

    assert_eq!(view.state, TaskState::Planning);
    assert!(
        repository
            .calls
            .contains(&"save_context_package_planning_transition")
    );
    assert_eq!(
        repository.consents.get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        )),
        None,
        "this path must never create a LegacyPhase4 consent"
    );
}

#[test]
fn start_context_package_planning_rejects_when_the_pair_is_not_prepared_without_writing_anything() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_planning(start_context_package_planning(task_id, 1))
        .expect_err("an unprepared task must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_planning_transition"),
        "a precondition failure must short-circuit before ever calling the repository"
    );
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn start_context_package_planning_rejects_wrong_state_without_calling_the_repository() {
    let (task, history) = restored_task(TaskState::Planning, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_planning(start_context_package_planning(task_id, 2))
        .expect_err("Planning must not be accepted as WorktreeReady");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_planning_transition")
    );
}

#[test]
fn start_context_package_planning_rejects_a_non_worktree_ready_isolation_without_calling_the_repository()
 {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut isolation = worktree_ready_isolation(task_id, 1);
    isolation.status = GitIsolationStatus::WorktreeCreating;
    repository.isolations.insert(task_id, isolation);
    seed_context_package_planning_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_planning(start_context_package_planning(task_id, 1))
        .expect_err("a non-WorktreeReady isolation must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_planning_transition")
    );
}

#[test]
fn start_context_package_planning_propagates_a_repository_failure_without_converting_it_to_success()
{
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "save_context_package_planning_transition",
            RepositoryErrorCode::InvalidAggregate,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    seed_context_package_planning_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_planning(start_context_package_planning(task_id, 1))
        .expect_err("a repository failure must propagate, never be treated as success");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::WorktreeReady,
        "a rejected write must leave the task exactly as it was"
    );
}

#[test]
fn context_package_planning_never_reuses_or_touches_a_legacy_phase4_planning_consent() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    let legacy_key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Planning,
        1,
        ContextDataScope::LegacyPhase4,
    );
    let legacy_consent = ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 90,
    };
    repository.consents.insert(legacy_key, legacy_consent);
    seed_context_package_planning_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    TaskService::new(&mut repository, &mut time)
        .start_context_package_planning(start_context_package_planning(task_id, 1))
        .expect("start context package planning succeeds independently of the legacy consent");

    assert_eq!(
        repository.consents.get(&legacy_key),
        Some(&legacy_consent),
        "the pre-existing LegacyPhase4 consent must be completely untouched"
    );
    assert_eq!(repository.consents.len(), 2, "both scopes coexist");
}

fn seed_context_package_implementation_pair(
    repository: &mut FakeRepository,
    task_id: TaskId,
    expected_version: u64,
    at_ms: i64,
) {
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Implementation,
        expected_version,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Implementation,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: at_ms,
        },
    );
    repository.context_package_manifests.insert(
        key,
        ContextPackageManifestRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Implementation,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            created_at_ms: at_ms,
        },
    );
}

fn seed_completed_planning_result(
    repository: &mut FakeRepository,
    task_id: TaskId,
    plan_text: &str,
) {
    repository.planning_results.insert(
        task_id,
        TaskPlanningResultRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            outcome: PlanningResultOutcome::Completed,
            exit_code: Some(0),
            turn_count: Some(5),
            started_at_ms: 135,
            completed_at_ms: 150,
            plan_text: Some(plan_text.to_owned()),
        },
    );
}

fn seed_task_brief(repository: &mut FakeRepository, task_id: TaskId) {
    repository.briefs.insert(
        task_id,
        TaskBriefRecord {
            task_id,
            requirements: "Add CSV export".to_owned(),
            completion_criteria: "Export button downloads a CSV".to_owned(),
            prohibited_scope: "Do not touch the import pipeline".to_owned(),
            created_at_ms: 10,
        },
    );
}

/// Seeds every structural precondition
/// `start_context_package_implementation` checks besides the Context
/// Package v1 pair itself: a `WorktreeReady` isolation, a `Completed`
/// non-empty stored Claude Planning result, and a `TaskBrief`.
fn seed_context_package_implementation_evidence(
    repository: &mut FakeRepository,
    task_id: TaskId,
    expected_version: u64,
) {
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, expected_version));
    seed_completed_planning_result(repository, task_id, "a stored plan");
    seed_task_brief(repository, task_id);
}

#[test]
fn context_package_implementation_readiness_is_true_only_when_both_consent_and_manifest_exist() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(20);

    let readiness = TaskService::new(&mut repository, &mut time)
        .get_context_package_implementation_readiness(task_id, 1)
        .expect("readiness lookup");

    assert!(readiness.ready);
}

#[test]
fn context_package_implementation_readiness_is_false_when_neither_consent_nor_manifest_exist() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let readiness = TaskService::new(&mut repository, &mut time)
        .get_context_package_implementation_readiness(task_id, 1)
        .expect("readiness lookup");

    assert!(!readiness.ready);
}

#[test]
fn context_package_implementation_readiness_is_a_fail_closed_error_on_a_partial_pair() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Implementation,
        1,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Implementation,
            approved_task_version: 1,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: 200,
        },
    );
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_context_package_implementation_readiness(task_id, 1)
        .expect_err("a consent-only partial pair must never be reported as ready: false");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn context_package_implementation_readiness_propagates_a_repository_failure_without_converting_it_to_ready_false()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "get_provider_consent",
            RepositoryErrorCode::DatabaseUnavailable,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_context_package_implementation_readiness(task_id, 1)
        .expect_err("a repository failure must propagate, never become ready: false");

    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
}

#[test]
fn start_context_package_implementation_commits_the_transition_when_everything_is_ready() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_context_package_implementation_evidence(&mut repository, task_id, 1);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let view = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect("start context package implementation");

    assert_eq!(view.state, TaskState::Implementing);
    assert!(
        repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
    assert_eq!(
        repository.consents.get(&(
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            1,
            ContextDataScope::LegacyPhase4,
        )),
        None,
        "this path must never create a LegacyPhase4 consent"
    );
}

#[test]
fn start_context_package_implementation_rejects_when_the_pair_is_not_prepared_without_writing_anything()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_context_package_implementation_evidence(&mut repository, task_id, 1);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("an unprepared task must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition"),
        "a precondition failure must short-circuit before ever calling the repository"
    );
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
}

#[test]
fn start_context_package_implementation_rejects_wrong_state_without_calling_the_repository() {
    let (task, history) = restored_task(TaskState::Implementing, 2, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 2))
        .expect_err("Implementing must not be accepted as AwaitingDesignApproval");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
}

#[test]
fn start_context_package_implementation_rejects_a_missing_isolation_without_calling_the_repository()
{
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_completed_planning_result(&mut repository, task_id, "a stored plan");
    seed_task_brief(&mut repository, task_id);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("a missing isolation must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
}

#[test]
fn start_context_package_implementation_rejects_a_non_worktree_ready_isolation_without_calling_the_repository()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut isolation = worktree_ready_isolation(task_id, 1);
    isolation.status = GitIsolationStatus::WorktreeCreating;
    repository.isolations.insert(task_id, isolation);
    seed_completed_planning_result(&mut repository, task_id, "a stored plan");
    seed_task_brief(&mut repository, task_id);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("a non-WorktreeReady isolation must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
}

#[test]
fn start_context_package_implementation_rejects_a_missing_stored_planning_result_without_calling_the_repository()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    seed_task_brief(&mut repository, task_id);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("a missing stored planning result must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
}

#[test]
fn start_context_package_implementation_rejects_a_non_completed_stored_planning_result_without_calling_the_repository()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    repository.planning_results.insert(
        task_id,
        TaskPlanningResultRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            outcome: PlanningResultOutcome::Cancelled,
            exit_code: None,
            turn_count: None,
            started_at_ms: 135,
            completed_at_ms: 150,
            plan_text: None,
        },
    );
    seed_task_brief(&mut repository, task_id);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("a non-Completed stored planning result must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
}

#[test]
fn start_context_package_implementation_rejects_a_missing_task_brief_without_calling_the_repository()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    seed_completed_planning_result(&mut repository, task_id, "a stored plan");
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("a missing TaskBrief must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(
        !repository
            .calls
            .contains(&"save_context_package_implementation_transition")
    );
}

#[test]
fn start_context_package_implementation_propagates_a_repository_failure_without_converting_it_to_success()
 {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "save_context_package_implementation_transition",
            RepositoryErrorCode::InvalidAggregate,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    seed_context_package_implementation_evidence(&mut repository, task_id, 1);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    let error = TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect_err("a repository failure must propagate, never be treated as success");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval,
        "a rejected write must leave the task exactly as it was"
    );
}

#[test]
fn context_package_implementation_never_reuses_or_touches_a_legacy_phase4_implementation_consent() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_context_package_implementation_evidence(&mut repository, task_id, 1);
    let legacy_key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Implementation,
        1,
        ContextDataScope::LegacyPhase4,
    );
    let legacy_consent = ProviderConsent {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: 1,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 90,
    };
    repository.consents.insert(legacy_key, legacy_consent);
    seed_context_package_implementation_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(210);

    TaskService::new(&mut repository, &mut time)
        .start_context_package_implementation(start_context_package_implementation(task_id, 1))
        .expect(
            "start context package implementation succeeds independently of the legacy consent",
        );

    assert_eq!(
        repository.consents.get(&legacy_key),
        Some(&legacy_consent),
        "the pre-existing LegacyPhase4 consent must be completely untouched"
    );
    assert_eq!(repository.consents.len(), 2, "both scopes coexist");
}

fn seed_context_package_review_pair(
    repository: &mut FakeRepository,
    task_id: TaskId,
    expected_version: u64,
    at_ms: i64,
) {
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Review,
        expected_version,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: at_ms,
        },
    );
    repository.context_package_manifests.insert(
        key,
        ContextPackageManifestRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            created_at_ms: at_ms,
        },
    );
}

#[test]
fn context_package_review_readiness_is_true_only_when_both_consent_and_manifest_exist() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    seed_context_package_review_pair(&mut repository, task_id, 1, 200);
    let mut time = FakeTime::at(20);

    let readiness = TaskService::new(&mut repository, &mut time)
        .get_context_package_review_readiness(task_id, 1)
        .expect("readiness lookup");

    assert!(readiness.ready);
}

#[test]
fn context_package_review_readiness_is_false_when_neither_consent_nor_manifest_exist() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let readiness = TaskService::new(&mut repository, &mut time)
        .get_context_package_review_readiness(task_id, 1)
        .expect("readiness lookup");

    assert!(!readiness.ready);
}

#[test]
fn context_package_review_readiness_is_a_fail_closed_error_on_a_partial_pair() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Review,
        1,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            approved_task_version: 1,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: 200,
        },
    );
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_context_package_review_readiness(task_id, 1)
        .expect_err("a consent-only partial pair must never be reported as ready: false");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn context_package_review_readiness_propagates_a_repository_failure_without_converting_it_to_ready_false()
 {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "get_provider_consent",
            RepositoryErrorCode::DatabaseUnavailable,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_context_package_review_readiness(task_id, 1)
        .expect_err("a repository failure must propagate, never become ready: false");

    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
}

#[test]
fn high_risk_approval_status_is_true_only_for_the_exact_task_version_and_category() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    repository.high_risk_approvals.insert(
        (task_id, 1, HighRiskCategory::DataMigration),
        chatoms_ports::repository::HighRiskApprovalRecord {
            task_id,
            approved_task_version: 1,
            risk_category: HighRiskCategory::DataMigration,
            approved_at_ms: 200,
        },
    );
    let mut time = FakeTime::at(20);

    let matching = TaskService::new(&mut repository, &mut time)
        .get_high_risk_approval_status(task_id, 1, HighRiskCategory::DataMigration)
        .expect("status lookup for the exact identity");
    assert!(matching.approved);

    let different_category = TaskService::new(&mut repository, &mut time)
        .get_high_risk_approval_status(task_id, 1, HighRiskCategory::ArchitectureChange)
        .expect("status lookup for a different category");
    assert!(!different_category.approved);
}

#[test]
fn high_risk_approval_status_is_false_when_no_approval_has_been_recorded() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let status = TaskService::new(&mut repository, &mut time)
        .get_high_risk_approval_status(task_id, 1, HighRiskCategory::DataMigration)
        .expect("status lookup");

    assert!(!status.approved);
}

#[test]
fn high_risk_approval_status_propagates_a_stale_version_instead_of_reporting_false() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_high_risk_approval_status(task_id, 0, HighRiskCategory::DataMigration)
        .expect_err("a stale expected_version must be rejected, never reported as approved: false");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn high_risk_approval_status_propagates_a_repository_failure_instead_of_reporting_false() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository {
        fail_on: Some((
            "get_high_risk_approval",
            RepositoryErrorCode::InvalidPersistenceState,
        )),
        ..FakeRepository::default()
    };
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let error = TaskService::new(&mut repository, &mut time)
        .get_high_risk_approval_status(task_id, 1, HighRiskCategory::DataMigration)
        .expect_err(
            "a repository failure (including a corrupted persisted category) must propagate, \
             never become approved: false",
        );

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
}

#[test]
fn approve_high_risk_operation_create_and_reuse_return_the_same_content_free_result() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let created = TaskService::new(&mut repository, &mut time)
        .approve_high_risk_operation(ApproveHighRiskOperationRequest::new(
            task_id,
            1,
            HighRiskCategory::DifficultToRecoverChange,
            210,
        ))
        .expect("first call creates the approval");
    let reused = TaskService::new(&mut repository, &mut time)
        .approve_high_risk_operation(ApproveHighRiskOperationRequest::new(
            task_id,
            1,
            HighRiskCategory::DifficultToRecoverChange,
            999,
        ))
        .expect("second call reuses the existing approval");

    assert_eq!(
        created, reused,
        "create and reuse must return the same content-free semantic result"
    );
    assert_eq!(
        created.approved_at_ms, 210,
        "the original timestamp must win"
    );
    assert_eq!(
        repository.high_risk_approvals.len(),
        1,
        "reuse must never insert a second row"
    );
}

#[test]
fn approve_high_risk_operation_never_mutates_task_state_version_history_or_lease() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    let before_task = repository.tasks.get(&task_id).cloned();
    let before_history_len = repository.transitions.get(&task_id).map(Vec::len);
    let before_lease = repository.active_lease;

    TaskService::new(&mut repository, &mut time)
        .approve_high_risk_operation(ApproveHighRiskOperationRequest::new(
            task_id,
            1,
            HighRiskCategory::SecurityPolicyChange,
            210,
        ))
        .expect("approve the operation");

    assert_eq!(repository.tasks.get(&task_id).cloned(), before_task);
    assert_eq!(
        repository.transitions.get(&task_id).map(Vec::len),
        before_history_len
    );
    assert_eq!(repository.active_lease, before_lease);
}

#[test]
fn approve_high_risk_operation_never_touches_provider_consent_manifest_or_validation_approval() {
    let (task, history) = restored_task(TaskState::Testing, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);

    TaskService::new(&mut repository, &mut time)
        .approve_high_risk_operation(ApproveHighRiskOperationRequest::new(
            task_id,
            1,
            HighRiskCategory::ExternalDataTransmissionAddition,
            210,
        ))
        .expect("approve the operation");
    TaskService::new(&mut repository, &mut time)
        .get_high_risk_approval_status(
            task_id,
            1,
            HighRiskCategory::ExternalDataTransmissionAddition,
        )
        .expect("read the status back");

    assert!(
        repository.consents.is_empty(),
        "no provider consent must be created"
    );
    assert!(
        repository.context_package_manifests.is_empty(),
        "no Context Package manifest must be created"
    );
    assert!(
        repository.validation_command_approvals.is_empty(),
        "no validation command approval must be created"
    );
    assert_eq!(
        repository.high_risk_approvals.len(),
        1,
        "exactly one high-risk approval must exist"
    );
}

#[test]
fn record_diff_approval_create_and_reuse_return_the_same_content_free_result() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);
    let hash = DiffContentHash::from_digest_bytes([1u8; 32]);

    let created = TaskService::new(&mut repository, &mut time)
        .record_diff_approval(RecordDiffApprovalRequest::new(task_id, 1, hash, 210))
        .expect("first call creates the approval");
    let reused = TaskService::new(&mut repository, &mut time)
        .record_diff_approval(RecordDiffApprovalRequest::new(task_id, 1, hash, 999))
        .expect("second call reuses the existing approval");

    assert_eq!(
        created, reused,
        "create and reuse must return the same content-free semantic result"
    );
    assert_eq!(
        created.approved_at_ms, 210,
        "the original timestamp must win"
    );
    assert_eq!(
        repository.diff_approvals.len(),
        1,
        "reuse must never insert a second row"
    );
}

#[test]
fn record_diff_approval_rejects_a_stale_version_without_writing_anything() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);
    let hash = DiffContentHash::from_digest_bytes([2u8; 32]);

    let error = TaskService::new(&mut repository, &mut time)
        .record_diff_approval(RecordDiffApprovalRequest::new(task_id, 0, hash, 210))
        .expect_err("a stale expected_version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert!(repository.diff_approvals.is_empty());
}

#[test]
fn record_diff_approval_never_mutates_task_state_version_history_or_lease() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);
    let hash = DiffContentHash::from_digest_bytes([3u8; 32]);

    let before_task = repository.tasks.get(&task_id).cloned();
    let before_history_len = repository.transitions.get(&task_id).map(Vec::len);
    let before_lease = repository.active_lease;

    TaskService::new(&mut repository, &mut time)
        .record_diff_approval(RecordDiffApprovalRequest::new(task_id, 1, hash, 210))
        .expect("record the approval");

    assert_eq!(repository.tasks.get(&task_id).cloned(), before_task);
    assert_eq!(
        repository.transitions.get(&task_id).map(Vec::len),
        before_history_len
    );
    assert_eq!(repository.active_lease, before_lease);
    assert_eq!(
        before_task.map(|task| task.state()),
        Some(TaskState::AwaitingUserDiffApproval),
        "recording a diff approval must never transition out of AwaitingUserDiffApproval, \
         and must never reach Merging or Completed"
    );
}

#[test]
fn record_diff_approval_never_touches_provider_consent_manifest_or_high_risk_approval() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut time = FakeTime::at(20);
    let hash = DiffContentHash::from_digest_bytes([4u8; 32]);

    TaskService::new(&mut repository, &mut time)
        .record_diff_approval(RecordDiffApprovalRequest::new(task_id, 1, hash, 210))
        .expect("record the approval");

    assert!(
        repository.consents.is_empty(),
        "no provider consent must be created"
    );
    assert!(
        repository.context_package_manifests.is_empty(),
        "no Context Package manifest must be created"
    );
    assert!(
        repository.high_risk_approvals.is_empty(),
        "no high-risk approval must be created"
    );
    assert_eq!(
        repository.diff_approvals.len(),
        1,
        "exactly one diff approval must exist"
    );
}

#[test]
fn reconcile_startup_merge_recovers_merging_and_post_merge_testing() {
    for state in [TaskState::Merging, TaskState::PostMergeTesting] {
        let (task, history) = restored_task(state, 4, 20, None);
        let task_id = task.id();
        let mut repository = FakeRepository::default();
        repository.seed_task(task, history);
        let mut time = FakeTime::at(30);

        let view = TaskService::new(&mut repository, &mut time)
            .reconcile_startup_merge()
            .expect("startup reconciliation succeeds")
            .expect("active merge work is recovered");

        assert_eq!(view.state, TaskState::RecoveryRequired);
        assert_eq!(view.version, 5);
        assert_eq!(
            repository.tasks[&task_id].state(),
            TaskState::RecoveryRequired
        );
        assert!(repository.active_lease.is_some());
    }
}
