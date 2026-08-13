mod support;

use std::str::FromStr;

use chatoms_application::{
    error::ApplicationErrorCode,
    tasks::{
        CreateTaskRequest, RecordPlanningResultRequest, StartPlanningRequest, TaskActionRequest,
        TaskService, TransitionTaskRequest,
    },
};
use chatoms_domain::{ProjectId, TaskId, TaskState, WorkKind};
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory, PortFailure},
    provider::ProviderKind,
    repository::{
        ActiveLease, GitIsolationStatus, PlanningResultOutcome, RepositoryErrorCode,
        TaskGitIsolation, TaskPlanningResultRecord,
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
        .get(&(task_id, ProviderKind::Claude, WorkKind::Planning, 1))
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
        consented_at_ms: 5,
    };
    repository.consents.insert(
        (task_id, ProviderKind::Claude, WorkKind::Planning, 1),
        existing_consent,
    );
    let mut time = FakeTime::at(20);

    let view = TaskService::new(&mut repository, &mut time)
        .start_planning(start_planning(task_id, 1))
        .expect("start planning reuses consent");

    assert_eq!(view.state, TaskState::Planning);
    let consent = repository
        .consents
        .get(&(task_id, ProviderKind::Claude, WorkKind::Planning, 1))
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
