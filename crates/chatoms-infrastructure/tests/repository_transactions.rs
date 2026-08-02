mod support;

use std::str::FromStr;

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, RecoveryValidation, ResumeValidation, Task, TaskId,
    TaskState, TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot,
};
use chatoms_infrastructure::database::{DatabaseConnection, SqliteFoundationRepository};
use chatoms_ports::repository::{FoundationRepository, RepositoryError, RepositoryErrorCode};
use rusqlite::params;

use support::{
    TestDatabase, count_rows, foreign_key_violation_count, insert_lease, insert_project,
    insert_task, is_constraint_error,
};

struct Fixture {
    database: TestDatabase,
    project_id: ProjectId,
}

impl Fixture {
    fn new() -> Self {
        let database = TestDatabase::migrated();
        let project_id = ProjectId::new();
        insert_project(&database.open_raw(), &project_id.to_string());
        Self {
            database,
            project_id,
        }
    }

    fn open(&self) -> DatabaseConnection {
        DatabaseConnection::open(self.database.path()).expect("open repository connection")
    }
}

fn initial_transition(task: &Task) -> TaskStateTransition {
    TaskStateTransition::initial(
        TaskStateTransitionId::new(),
        task.id(),
        ActorKind::from_str("application").expect("valid actor"),
        ReasonCode::from_str("task.created").expect("valid reason"),
        task.created_at_ms(),
    )
}

fn transition(
    id: TaskStateTransitionId,
    task: &Task,
    from_state: TaskState,
    occurred_at_ms: i64,
) -> TaskStateTransition {
    TaskStateTransition::new(TaskStateTransitionSnapshot {
        id,
        task_id: task.id(),
        sequence: task.version() + 1,
        from_state: Some(from_state),
        to_state: task.state(),
        task_version: task.version(),
        actor_kind: ActorKind::from_str("application").expect("valid actor"),
        reason_code: ReasonCode::from_str("task.transition").expect("valid reason"),
        occurred_at_ms,
    })
    .expect("valid transition record")
}

fn create_task(
    repository: &mut impl FoundationRepository,
    project_id: ProjectId,
) -> (Task, TaskStateTransition) {
    let task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create task transaction");
    (task, initial)
}

fn advance(
    repository: &mut impl FoundationRepository,
    task: &mut Task,
    next: TaskState,
    occurred_at_ms: i64,
) -> TaskStateTransition {
    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(next, occurred_at_ms)
        .expect("domain transition");
    let record = transition(
        TaskStateTransitionId::new(),
        task,
        previous_state,
        occurred_at_ms,
    );
    if next.is_terminal() {
        repository
            .terminate_task(expected_version, task, &record)
            .expect("terminal repository transaction");
    } else {
        repository
            .save_transition(expected_version, task, &record)
            .expect("repository transition transaction");
    }
    record
}

fn assert_code(error: RepositoryError, code: RepositoryErrorCode) {
    assert_eq!(error.code(), code, "unexpected repository error: {error}");
}

#[test]
fn task_creation_persists_task_initial_transition_lease_and_read_models() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, initial) = create_task(&mut repository, fixture.project_id);

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    let transitions = repository
        .list_task_transitions(task.id())
        .expect("list transitions");
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].id(), initial.id());
    assert_eq!(transitions[0].sequence(), 1);
    assert_eq!(transitions[0].from_state(), None);
    assert_eq!(transitions[0].to_state(), TaskState::Created);
    assert_eq!(transitions[0].task_version(), 0);

    let lease = repository
        .active_lease()
        .expect("read lease")
        .expect("active lease");
    assert_eq!(lease.task_id, task.id());
    assert_eq!(lease.acquired_at_ms, 100);

    let projects = repository.list_projects().expect("list projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, fixture.project_id);

    let raw = fixture.database.open_raw();
    assert_eq!(count_rows(&raw, "tasks"), 1);
    assert_eq!(count_rows(&raw, "task_state_transitions"), 1);
    assert_eq!(count_rows(&raw, "active_task_leases"), 1);
}

#[test]
fn create_rejects_missing_project_duplicate_active_task_and_invalid_inputs() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);

    let missing_project_task = Task::new(TaskId::new(), ProjectId::new(), 100);
    let missing_initial = initial_transition(&missing_project_task);
    assert_code(
        repository
            .create_task(&missing_project_task, &missing_initial, 100)
            .expect_err("missing project must fail"),
        RepositoryErrorCode::ProjectNotFound,
    );

    let (task, initial) = create_task(&mut repository, fixture.project_id);
    assert_code(
        repository
            .create_task(&task, &initial, 100)
            .expect_err("duplicate task must fail"),
        RepositoryErrorCode::DuplicateTask,
    );

    let second = Task::new(TaskId::new(), fixture.project_id, 100);
    let second_initial = initial_transition(&second);
    assert_code(
        repository
            .create_task(&second, &second_initial, 100)
            .expect_err("second active task must fail"),
        RepositoryErrorCode::ActiveLeaseConflict,
    );

    let mismatch = TaskStateTransition::initial(
        TaskStateTransitionId::new(),
        TaskId::new(),
        ActorKind::from_str("application").expect("actor"),
        ReasonCode::from_str("task.created").expect("reason"),
        100,
    );
    let invalid_task = Task::new(TaskId::new(), fixture.project_id, 100);
    assert_code(
        repository
            .create_task(&invalid_task, &mismatch, 100)
            .expect_err("transition task mismatch"),
        RepositoryErrorCode::InvalidAggregate,
    );

    let mut terminal_task = Task::new(TaskId::new(), fixture.project_id, 100);
    terminal_task
        .transition_to(TaskState::Failed, 101)
        .expect("Created -> Failed");
    assert_code(
        repository
            .create_task(&terminal_task, &initial_transition(&terminal_task), 101)
            .expect_err("terminal create must fail"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn creation_transition_failure_and_lease_conflict_leave_no_partial_task() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut first, first_initial) = create_task(&mut repository, fixture.project_id);
    advance(&mut repository, &mut first, TaskState::Failed, 101);
    assert!(
        repository
            .active_lease()
            .expect("lease after terminal")
            .is_none()
    );

    let second = Task::new(TaskId::new(), fixture.project_id, 200);
    let duplicate_transition_id = TaskStateTransition::initial(
        first_initial.id(),
        second.id(),
        ActorKind::from_str("application").expect("actor"),
        ReasonCode::from_str("task.created").expect("reason"),
        200,
    );
    assert_code(
        repository
            .create_task(&second, &duplicate_transition_id, 200)
            .expect_err("duplicate transition ID must rollback"),
        RepositoryErrorCode::OperationFailed,
    );
    assert_eq!(
        repository
            .get_task(second.id())
            .expect("read rolled back task"),
        None
    );
    assert!(
        repository
            .active_lease()
            .expect("lease remains empty")
            .is_none()
    );

    let third = Task::new(TaskId::new(), fixture.project_id, 300);
    let invalid_sequence = TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id: third.id(),
        sequence: 2,
        from_state: Some(TaskState::Created),
        to_state: TaskState::ProjectValidated,
        task_version: 1,
        actor_kind: ActorKind::from_str("application").expect("actor"),
        reason_code: ReasonCode::from_str("invalid.initial").expect("reason"),
        occurred_at_ms: 300,
    })
    .expect("valid noninitial record");
    assert_code(
        repository
            .create_task(&third, &invalid_sequence, 300)
            .expect_err("noninitial record must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn general_transition_updates_version_sequence_and_round_trips() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    let record = advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);

    assert_eq!(task.version(), 1);
    assert_eq!(record.sequence(), 2);
    assert_eq!(record.from_state(), Some(TaskState::Created));
    assert_eq!(record.to_state(), TaskState::ProjectValidated);
    assert_eq!(record.task_version(), 1);
    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("list history")
            .len(),
        2
    );
}

#[test]
fn version_sequence_transition_and_immutable_field_conflicts_rollback() {
    let fixture = Fixture::new();
    let other_project = ProjectId::new();
    insert_project(&fixture.database.open_raw(), &other_project.to_string());
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, initial) = create_task(&mut repository, fixture.project_id);

    let mut next = task.clone();
    next.transition_to(TaskState::ProjectValidated, 110)
        .expect("domain transition");
    let valid_record = transition(TaskStateTransitionId::new(), &next, TaskState::Created, 110);
    assert_code(
        repository
            .save_transition(1, &next, &valid_record)
            .expect_err("wrong expected version"),
        RepositoryErrorCode::VersionConflict,
    );

    let wrong_sequence = TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id: next.id(),
        sequence: 3,
        from_state: Some(TaskState::Created),
        to_state: TaskState::ProjectValidated,
        task_version: 1,
        actor_kind: ActorKind::from_str("application").expect("actor"),
        reason_code: ReasonCode::from_str("task.transition").expect("reason"),
        occurred_at_ms: 110,
    })
    .expect("domain transition record");
    assert_code(
        repository
            .save_transition(0, &next, &wrong_sequence)
            .expect_err("sequence gap"),
        RepositoryErrorCode::TransitionSequenceConflict,
    );

    let wrong_to = TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id: next.id(),
        sequence: 2,
        from_state: Some(TaskState::Created),
        to_state: TaskState::WorktreeCreating,
        task_version: 1,
        actor_kind: ActorKind::from_str("application").expect("actor"),
        reason_code: ReasonCode::from_str("task.transition").expect("reason"),
        occurred_at_ms: 110,
    })
    .expect("domain transition record");
    assert_code(
        repository
            .save_transition(0, &next, &wrong_to)
            .expect_err("aggregate and record mismatch"),
        RepositoryErrorCode::InvalidAggregate,
    );

    let mut invalid_edge_snapshot = next.snapshot();
    invalid_edge_snapshot.state = TaskState::Testing;
    let invalid_edge = Task::restore(invalid_edge_snapshot).expect("individually valid aggregate");
    let invalid_edge_record = transition(
        TaskStateTransitionId::new(),
        &invalid_edge,
        TaskState::Created,
        110,
    );
    assert_code(
        repository
            .save_transition(0, &invalid_edge, &invalid_edge_record)
            .expect_err("static transition policy cannot be bypassed by restoration"),
        RepositoryErrorCode::InvalidAggregate,
    );

    let mut changed_project_snapshot = next.snapshot();
    changed_project_snapshot.project_id = other_project;
    let changed_project = Task::restore(changed_project_snapshot).expect("valid aggregate");
    assert_code(
        repository
            .save_transition(0, &changed_project, &valid_record)
            .expect_err("project identity changed"),
        RepositoryErrorCode::InvalidAggregate,
    );

    let duplicate_record = transition(initial.id(), &next, TaskState::Created, 110);
    assert_code(
        repository
            .save_transition(0, &next, &duplicate_record)
            .expect_err("transition insert failure"),
        RepositoryErrorCode::OperationFailed,
    );
    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(task)
    );
    assert_eq!(
        repository
            .list_task_transitions(next.id())
            .expect("history after rollback")
            .len(),
        1
    );
}

#[test]
fn repository_rejects_timestamp_regression_against_persisted_task() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 200);

    let mut snapshot = task.snapshot();
    snapshot.state = TaskState::WorktreeCreating;
    snapshot.version = 2;
    snapshot.updated_at_ms = 150;
    let regressed = Task::restore(snapshot).expect("domain-monotonic snapshot");
    let record = transition(
        TaskStateTransitionId::new(),
        &regressed,
        TaskState::ProjectValidated,
        150,
    );
    assert_code(
        repository
            .save_transition(1, &regressed, &record)
            .expect_err("persistence timestamp regression"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        repository
            .get_task(task.id())
            .expect("task after rejection"),
        Some(task)
    );
}

fn drive_to_post_merge_testing(repository: &mut impl FoundationRepository, task: &mut Task) {
    for (index, state) in [
        TaskState::ProjectValidated,
        TaskState::WorktreeCreating,
        TaskState::WorktreeReady,
        TaskState::PlanningWithClaude,
        TaskState::ImplementingWithCodex,
        TaskState::Testing,
        TaskState::ReviewingWithClaude,
        TaskState::AwaitingUserDiffApproval,
        TaskState::Merging,
        TaskState::PostMergeTesting,
    ]
    .into_iter()
    .enumerate()
    {
        advance(repository, task, state, 110 + index as i64);
    }
}

#[test]
fn terminal_transitions_are_atomic_for_completed_failed_and_cancelled() {
    for terminal in [
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        let fixture = Fixture::new();
        let mut connection = fixture.open();
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        let (mut task, _) = create_task(&mut repository, fixture.project_id);
        if terminal == TaskState::Completed {
            drive_to_post_merge_testing(&mut repository, &mut task);
        }
        let expected_version = task.version();
        let previous = task.state();
        task.transition_to(terminal, 200)
            .expect("domain terminal transition");
        let record = transition(TaskStateTransitionId::new(), &task, previous, 200);
        repository
            .terminate_task(expected_version, &task, &record)
            .expect("terminal transaction");

        let restored = repository
            .get_task(task.id())
            .expect("read terminal task")
            .expect("terminal task exists");
        assert_eq!(restored, task);
        assert_eq!(restored.terminal_at_ms(), Some(200));
        assert_eq!(restored.resume_target_state(), None);
        assert!(repository.active_lease().expect("read lease").is_none());
        assert_eq!(foreign_key_violation_count(&fixture.database.open_raw()), 0);
    }
}

#[test]
fn terminal_transition_insert_failure_rolls_back_task_and_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    let before = task.clone();
    task.transition_to(TaskState::Failed, 110)
        .expect("Created -> Failed");
    let duplicate = transition(initial.id(), &task, TaskState::Created, 110);
    assert_code(
        repository
            .terminate_task(0, &task, &duplicate)
            .expect_err("duplicate transition insert must fail"),
        RepositoryErrorCode::OperationFailed,
    );
    assert_eq!(
        repository.get_task(task.id()).expect("restored task"),
        Some(before)
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease after rollback")
            .map(|lease| lease.task_id),
        Some(task.id())
    );
}

#[test]
fn post_terminal_transitions_preserve_terminal_time_and_never_acquire_lease() {
    for terminal in [
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        let fixture = Fixture::new();
        let mut connection = fixture.open();
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        let (mut task, _) = create_task(&mut repository, fixture.project_id);
        if terminal == TaskState::Completed {
            drive_to_post_merge_testing(&mut repository, &mut task);
        }
        advance(&mut repository, &mut task, terminal, 200);
        let terminal_at = task.terminal_at_ms();
        let post_state = if terminal == TaskState::Cancelled {
            TaskState::Archived
        } else {
            TaskState::CleanupPending
        };
        advance(&mut repository, &mut task, post_state, 210);
        if post_state == TaskState::CleanupPending {
            advance(&mut repository, &mut task, TaskState::Archived, 220);
        }
        assert_eq!(task.terminal_at_ms(), terminal_at);
        assert_eq!(task.state(), TaskState::Archived);
        assert!(
            repository
                .active_lease()
                .expect("no post-terminal lease")
                .is_none()
        );
        assert_eq!(
            repository
                .list_task_transitions(task.id())
                .expect("post-terminal history")
                .last()
                .expect("last transition")
                .task_version(),
            task.version()
        );
    }
}

#[test]
fn pause_resume_and_recovery_context_are_persisted_consistently() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);

    let mut invalid_pause_snapshot = task.snapshot();
    invalid_pause_snapshot.state = TaskState::Paused;
    invalid_pause_snapshot.version += 1;
    invalid_pause_snapshot.resume_target_state = Some(TaskState::Testing);
    invalid_pause_snapshot.updated_at_ms = 135;
    let invalid_pause =
        Task::restore(invalid_pause_snapshot).expect("individually valid paused aggregate");
    let invalid_pause_record = transition(
        TaskStateTransitionId::new(),
        &invalid_pause,
        TaskState::WorktreeReady,
        135,
    );
    assert_code(
        repository
            .save_transition(task.version(), &invalid_pause, &invalid_pause_record)
            .expect_err("pause target must match the persisted source state"),
        RepositoryErrorCode::InvalidAggregate,
    );

    let expected = task.version();
    let from = task.state();
    task.pause(140).expect("pause worktree-ready task");
    let pause_record = transition(TaskStateTransitionId::new(), &task, from, 140);
    repository
        .save_transition(expected, &task, &pause_record)
        .expect("persist pause");
    assert_eq!(
        repository
            .get_task(task.id())
            .expect("read paused")
            .expect("paused task")
            .resume_target_state(),
        Some(TaskState::WorktreeReady)
    );

    let expected = task.version();
    task.resume_from_pause(
        TaskState::WorktreeReady,
        ResumeValidation::from_completed_checks(),
        150,
    )
    .expect("resume pause");
    let resume_record = transition(TaskStateTransitionId::new(), &task, TaskState::Paused, 150);
    repository
        .save_transition(expected, &task, &resume_record)
        .expect("persist resume");
    assert_eq!(task.resume_target_state(), None);

    advance(
        &mut repository,
        &mut task,
        TaskState::PlanningWithClaude,
        160,
    );
    advance(&mut repository, &mut task, TaskState::RecoveryRequired, 170);
    assert_eq!(task.resume_target_state(), None);
    task.set_recovery_target(
        TaskState::PlanningWithClaude,
        RecoveryValidation::from_completed_checks(),
    )
    .expect("set recovery target");
    repository
        .save_recovery_target(task.version(), &task)
        .expect("persist recovery target");
    assert_eq!(
        repository
            .get_task(task.id())
            .expect("read recovery")
            .expect("recovery task")
            .resume_target_state(),
        Some(TaskState::PlanningWithClaude)
    );

    let expected = task.version();
    task.pause_from_recovery(RecoveryValidation::from_completed_checks(), 180)
        .expect("pause from recovery");
    let recovery_pause = transition(
        TaskStateTransitionId::new(),
        &task,
        TaskState::RecoveryRequired,
        180,
    );
    repository
        .save_transition(expected, &task, &recovery_pause)
        .expect("persist recovery pause");
    assert_eq!(
        task.resume_target_state(),
        Some(TaskState::PlanningWithClaude)
    );

    let expected = task.version();
    task.resume_from_pause(
        TaskState::PlanningWithClaude,
        ResumeValidation::from_completed_checks(),
        190,
    )
    .expect("resume verified target");
    let final_resume = transition(TaskStateTransitionId::new(), &task, TaskState::Paused, 190);
    repository
        .save_transition(expected, &task, &final_resume)
        .expect("persist final resume");
    assert_eq!(task.resume_target_state(), None);
    assert_eq!(
        repository.get_task(task.id()).expect("roundtrip"),
        Some(task)
    );
}

#[test]
fn schema_and_repository_reject_malformed_persistence_rows_and_history_gaps() {
    let fixture = Fixture::new();
    let mut raw = fixture.database.open_raw();

    let transaction = raw.transaction().expect("begin invalid row");
    let error = insert_task(
        &transaction,
        "invalid-state-task",
        &fixture.project_id.to_string(),
        "InvalidState",
        0,
        None,
        None,
    )
    .expect_err("schema rejects invalid state");
    assert!(is_constraint_error(&error));
    transaction.rollback().expect("rollback invalid state");

    let bad_project_id = "not-a-uuid";
    insert_project(&raw, bad_project_id);
    let valid_task_id = TaskId::new();
    let transaction = raw.transaction().expect("begin malformed aggregate");
    insert_task(
        &transaction,
        &valid_task_id.to_string(),
        bad_project_id,
        "Created",
        0,
        None,
        None,
    )
    .expect("schema permits domain-invalid project ID");
    transaction
        .execute(
            "INSERT INTO task_state_transitions (
                id, task_id, sequence, from_state, to_state, task_version,
                actor_kind, reason_code, occurred_at_ms
             ) VALUES (?1, ?2, 1, NULL, 'Created', 0, 'application', 'task.created', 100)",
            params![
                TaskStateTransitionId::new().to_string(),
                valid_task_id.to_string()
            ],
        )
        .expect("insert malformed task initial transition");
    insert_lease(&transaction, &valid_task_id.to_string()).expect("insert malformed task lease");
    transaction
        .commit()
        .expect("commit malformed aggregate fixture");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    assert_code(
        repository
            .get_task(valid_task_id)
            .expect_err("invalid project UUID must fail restore"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    let gap_fixture = Fixture::new();
    let mut connection = gap_fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, gap_fixture.project_id);
    let mut raw = gap_fixture.database.open_raw();
    let transaction = raw.transaction().expect("begin gap fixture");
    transaction
        .execute(
            "UPDATE tasks SET state = 'ProjectValidated', version = 1, updated_at_ms = 110
             WHERE id = ?1",
            [task.id().to_string()],
        )
        .expect("update gap task");
    transaction
        .execute(
            "INSERT INTO task_state_transitions (
                id, task_id, sequence, from_state, to_state, task_version,
                actor_kind, reason_code, occurred_at_ms
             ) VALUES (?1, ?2, 3, 'Created', 'ProjectValidated', 1,
                       'application', 'task.transition', 110)",
            params![
                TaskStateTransitionId::new().to_string(),
                task.id().to_string()
            ],
        )
        .expect("insert sequence gap");
    transaction.commit().expect("commit gap fixture");
    drop(raw);
    let mut connection = gap_fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    assert_code(
        repository
            .list_task_transitions(task.id())
            .expect_err("history gap must be detected"),
        RepositoryErrorCode::InvalidPersistenceState,
    );
}
