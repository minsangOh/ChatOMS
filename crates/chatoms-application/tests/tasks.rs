mod support;

use std::str::FromStr;

use chatoms_application::{
    error::ApplicationErrorCode,
    tasks::{CreateTaskRequest, TaskActionRequest, TaskService, TransitionTaskRequest},
};
use chatoms_domain::{ProjectId, TaskId, TaskState};
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory, PortFailure},
    repository::{ActiveLease, RepositoryErrorCode},
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
