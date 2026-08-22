mod support;

use std::str::FromStr;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use chatoms_domain::{
    ActorKind, ContextDataScope, HighRiskCategory, ProjectId, ReasonCode, RecoveryValidation,
    ResumeValidation, Task, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot, ValidationCommandKind, ValidationExecutionScope, WorkKind,
};
use chatoms_infrastructure::database::{DatabaseConnection, SqliteFoundationRepository};
use chatoms_ports::diff::DiffContentHash;
use chatoms_ports::manual_merge_resolution::ManualResolutionDigest;
use chatoms_ports::provider::ProviderKind;
use chatoms_ports::repository::{
    ContextPackageManifestRecord, DiffApprovalRecord, FoundationRepository, GitIsolationStatus,
    GitOperationReceiptKind, HighRiskApprovalRecord, ImplementationResultOutcome,
    ManualMergeResolutionConfirmationRecord, MergeAbortApprovalRecord, PlanningResultOutcome,
    PostMergeValidationResultAttempt, PostMergeValidationResultOutcome, ProviderConsent,
    RepositoryError, RepositoryErrorCode, ReviewResultOutcome, TaskBriefRecord, TaskGitIsolation,
    TaskImplementationResultRecord, TaskPlanningResultRecord, TaskReviewResultRecord,
    ValidationCommandApprovalRecord, ValidationCommandResultAttempt,
    ValidationCommandResultOutcome,
};
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

fn advance_to_worktree_ready(repository: &mut impl FoundationRepository, task: &mut Task) {
    advance(repository, task, TaskState::ProjectValidated, 110);
    advance(repository, task, TaskState::WorktreeCreating, 120);
    advance(repository, task, TaskState::WorktreeReady, 130);
}

fn advance_to_planning(repository: &mut impl FoundationRepository, task: &mut Task) {
    advance_to_worktree_ready(repository, task);
    advance(repository, task, TaskState::Planning, 140);
}

fn advance_to_awaiting_design_approval(
    repository: &mut impl FoundationRepository,
    task: &mut Task,
) {
    advance_to_planning(repository, task);
    advance(repository, task, TaskState::AwaitingDesignApproval, 150);
}

fn advance_to_implementing(repository: &mut impl FoundationRepository, task: &mut Task) {
    advance_to_awaiting_design_approval(repository, task);
    advance(repository, task, TaskState::Implementing, 160);
}

fn advance_to_testing(repository: &mut impl FoundationRepository, task: &mut Task) {
    advance_to_implementing(repository, task);
    advance(repository, task, TaskState::Testing, 170);
}

fn advance_to_reviewing(repository: &mut impl FoundationRepository, task: &mut Task) {
    advance_to_testing(repository, task);
    advance(repository, task, TaskState::Reviewing, 180);
}

fn advance_to_awaiting_user_diff_approval(
    repository: &mut impl FoundationRepository,
    task: &mut Task,
) {
    advance_to_reviewing(repository, task);
    advance(repository, task, TaskState::AwaitingUserDiffApproval, 190);
}

fn advance_to_merge_conflict(repository: &mut impl FoundationRepository, task: &mut Task) {
    advance_to_awaiting_user_diff_approval(repository, task);
    advance(repository, task, TaskState::Merging, 200);
    advance(repository, task, TaskState::MergeConflict, 210);
}

fn project_root_validation_approval(
    repository: &mut impl FoundationRepository,
    task: &Task,
    kind: ValidationCommandKind,
) -> ValidationCommandApprovalRecord {
    let identity = repository
        .get_project_identity(task.project_id())
        .expect("read project identity")
        .expect("project identity exists");
    let mut approval = validation_command_approval(
        task.id(),
        task.version(),
        kind,
        "cargo",
        match kind {
            ValidationCommandKind::Test => &["test", "--workspace"],
            ValidationCommandKind::Build => &["build", "--workspace"],
            _ => &[],
        },
    );
    approval.execution_scope = ValidationExecutionScope::ProjectRoot;
    approval.target_project_id = Some(task.project_id());
    approval.target_project_identity_revision = Some(identity.revision);
    approval.target_root_volume_serial_hex = Some(identity.root_volume_serial_hex);
    approval.target_root_file_id_hex = Some(identity.root_file_id_hex);
    approval
}

fn completed_planning_result(task_id: TaskId, plan_text: &str) -> TaskPlanningResultRecord {
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
    }
}

#[test]
fn isolation_task_intent_and_domain_transition_commit_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = Task::new(TaskId::new(), fixture.project_id, 100);
    let initial = initial_transition(&task);
    task.transition_to(TaskState::ProjectValidated, 101)
        .expect("classify project");
    let classified = transition(TaskStateTransitionId::new(), &task, TaskState::Created, 101);
    let mut isolation = TaskGitIsolation {
        task_id: task.id(),
        project_id: fixture.project_id,
        status: GitIsolationStatus::Ready,
        operation_id: None,
        expected_task_version: 1,
        base_branch: None,
        base_commit: None,
        worktree_path: None,
        branch_created_by_app: false,
        worktree_created_by_app: false,
        created_at_ms: 100,
        updated_at_ms: 101,
    };
    repository
        .create_isolation_task(&task, &initial, &classified, 100, &isolation, None)
        .expect("atomic isolation task");
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .len(),
        2
    );
    assert_eq!(
        repository.get_task_isolation(task.id()).expect("isolation"),
        Some(isolation.clone())
    );

    let previous = task.state();
    task.transition_to(TaskState::WorktreeCreating, 102)
        .expect("worktree intent state");
    let worktree_transition = transition(TaskStateTransitionId::new(), &task, previous, 102);
    isolation.status = GitIsolationStatus::WorktreeCreating;
    isolation.operation_id = Some(chatoms_domain::GitOperationId::new());
    isolation.expected_task_version = task.version();
    isolation.base_branch = Some("main".to_owned());
    isolation.base_commit = Some("a".repeat(40));
    isolation.worktree_path = Some("C:/managed/project/task".to_owned());
    isolation.updated_at_ms = 102;
    repository
        .save_isolation_transition(1, &task, &worktree_transition, &isolation)
        .expect("atomic Git intent transition");
    let operation_id = isolation.operation_id.expect("operation id");
    for kind in [
        GitOperationReceiptKind::CommandStarted,
        GitOperationReceiptKind::CommandSucceeded,
        GitOperationReceiptKind::PostVerified,
    ] {
        repository
            .append_git_operation_receipt(operation_id, kind, None, 102)
            .expect("durable operation receipt");
    }
    assert_eq!(
        repository.get_task(task.id()).expect("task"),
        Some(task.clone())
    );
    assert_eq!(
        repository.get_task_isolation(task.id()).expect("isolation"),
        Some(isolation.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .len(),
        3
    );

    let previous = task.state();
    task.transition_to(TaskState::WorktreeReady, 103)
        .expect("worktree ready state");
    let ready_transition = transition(TaskStateTransitionId::new(), &task, previous, 103);
    isolation.status = GitIsolationStatus::WorktreeReady;
    isolation.expected_task_version = task.version();
    isolation.branch_created_by_app = true;
    isolation.worktree_created_by_app = true;
    isolation.updated_at_ms = 103;
    repository
        .save_worktree_completion(2, &task, &ready_transition, &isolation)
        .expect("receipt, transition, and isolation complete atomically");
    assert!(
        repository
            .list_incomplete_git_operations()
            .expect("incomplete attempts")
            .is_empty()
    );
    let receipts = repository
        .list_git_operation_receipts(operation_id)
        .expect("operation receipts");
    assert_eq!(receipts.len(), 4);
    assert_eq!(
        receipts.last().map(|receipt| receipt.kind),
        Some(GitOperationReceiptKind::CompletionRecorded)
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id())
    );
}

#[test]
fn task_brief_persists_atomically_with_task_creation_and_is_immutable() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = Task::new(TaskId::new(), fixture.project_id, 100);
    let initial = initial_transition(&task);
    let mut classified_task = task.clone();
    classified_task
        .transition_to(TaskState::ProjectValidated, 101)
        .expect("classify project");
    let classified = transition(
        TaskStateTransitionId::new(),
        &classified_task,
        TaskState::Created,
        101,
    );
    let isolation = TaskGitIsolation {
        task_id: task.id(),
        project_id: fixture.project_id,
        status: GitIsolationStatus::Ready,
        operation_id: None,
        expected_task_version: 1,
        base_branch: None,
        base_commit: None,
        worktree_path: None,
        branch_created_by_app: false,
        worktree_created_by_app: false,
        created_at_ms: 100,
        updated_at_ms: 101,
    };
    let brief = TaskBriefRecord {
        task_id: task.id(),
        requirements: "Add CSV export".to_owned(),
        completion_criteria: "Export button downloads a CSV".to_owned(),
        prohibited_scope: "Do not touch the import pipeline".to_owned(),
        created_at_ms: 100,
    };
    repository
        .create_isolation_task(
            &classified_task,
            &initial,
            &classified,
            100,
            &isolation,
            Some(&brief),
        )
        .expect("atomic isolation task with brief");

    assert_eq!(
        repository.get_task_brief(task.id()).expect("brief"),
        Some(brief)
    );

    let raw = fixture.database.open_raw();
    let update_error = raw
        .execute(
            "UPDATE task_briefs SET requirements = 'changed' WHERE task_id = ?1",
            [task.id().to_string()],
        )
        .expect_err("task_briefs must be immutable");
    assert!(is_constraint_error(&update_error));
    let delete_error = raw
        .execute(
            "DELETE FROM task_briefs WHERE task_id = ?1",
            [task.id().to_string()],
        )
        .expect_err("task_briefs rows must not be deletable");
    assert!(is_constraint_error(&delete_error));
}

#[test]
fn task_creation_without_brief_leaves_no_task_brief_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = Task::new(TaskId::new(), fixture.project_id, 100);
    let initial = initial_transition(&task);
    let mut classified_task = task.clone();
    classified_task
        .transition_to(TaskState::ProjectValidated, 101)
        .expect("classify project");
    let classified = transition(
        TaskStateTransitionId::new(),
        &classified_task,
        TaskState::Created,
        101,
    );
    let isolation = TaskGitIsolation {
        task_id: task.id(),
        project_id: fixture.project_id,
        status: GitIsolationStatus::Ready,
        operation_id: None,
        expected_task_version: 1,
        base_branch: None,
        base_commit: None,
        worktree_path: None,
        branch_created_by_app: false,
        worktree_created_by_app: false,
        created_at_ms: 100,
        updated_at_ms: 101,
    };
    repository
        .create_isolation_task(
            &classified_task,
            &initial,
            &classified,
            100,
            &isolation,
            None,
        )
        .expect("atomic isolation task without brief");

    assert_eq!(
        repository.get_task_brief(task.id()).expect("no brief"),
        None
    );
    assert_eq!(count_rows(&fixture.database.open_raw(), "task_briefs"), 0);
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
fn save_planning_transition_persists_consent_state_and_history_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let expected_version = task.version();

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 140,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 140);

    repository
        .save_planning_transition(expected_version, &task, &record, Some(&consent))
        .expect("atomic planning transition");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record
    );
    let stored = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Planning,
            expected_version,
            ContextDataScope::LegacyPhase4,
        )
        .expect("read consent")
        .expect("consent persisted");
    assert_eq!(stored, consent);
}

#[test]
fn save_planning_transition_without_a_consent_writes_no_consent_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 140);

    repository
        .save_planning_transition(expected_version, &task, &record, None)
        .expect("reused-consent planning transition");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent"),
        None,
        "no consent row must be written when reusing an existing grant"
    );
}

#[test]
fn save_planning_transition_rolls_back_consent_when_history_insert_fails() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 140,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    // Reusing an already-persisted transition id forces the final history
    // insert to fail on its primary key after the consent row has already
    // been written inside the same transaction, proving the whole write
    // rolls back together rather than leaving the consent behind.
    let duplicate_id_record = transition(initial.id(), &task, previous_state, 140);

    assert_code(
        repository
            .save_planning_transition(
                expected_version,
                &task,
                &duplicate_id_record,
                Some(&consent),
            )
            .expect_err("duplicate transition id must fail"),
        RepositoryErrorCode::OperationFailed,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before)
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent after rollback"),
        None,
        "consent insert must roll back with the rest of the transaction"
    );
}

#[test]
fn save_planning_transition_rejects_version_mismatch_without_touching_consent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let before = task.clone();
    let stale_expected_version = before.version() + 41;

    // The caller-declared expected_version and the consent's
    // approved_task_version agree with each other (as a real caller would
    // build them), but both are stale relative to the task's actual
    // persisted version, so the mismatch must be caught against the
    // database, not against the consent argument itself.
    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: stale_expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 140,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 140);

    assert_code(
        repository
            .save_planning_transition(stale_expected_version, &task, &record, Some(&consent))
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                consent.approved_task_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent"),
        None
    );
}

#[test]
fn save_implementation_transition_persists_consent_state_and_history_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let expected_version = task.version();

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 160,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 160);

    repository
        .save_implementation_transition(expected_version, &task, &record, Some(&consent))
        .expect("atomic implementation transition");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record
    );
    let stored = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Implementation,
            expected_version,
            ContextDataScope::LegacyPhase4,
        )
        .expect("read consent")
        .expect("consent persisted");
    assert_eq!(stored, consent);
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Implementing is not terminal and must keep the lease"
    );
}

#[test]
fn save_implementation_transition_without_a_consent_writes_no_consent_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 160);

    repository
        .save_implementation_transition(expected_version, &task, &record, None)
        .expect("reused-consent implementation transition");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent"),
        None,
        "no consent row must be written when reusing an existing grant"
    );
}

#[test]
fn save_implementation_transition_rolls_back_consent_when_history_insert_fails() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 160,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    // Reusing an already-persisted transition id forces the final history
    // insert to fail on its primary key after the consent row has already
    // been written inside the same transaction, proving the whole write
    // rolls back together rather than leaving the consent behind.
    let duplicate_id_record = transition(initial.id(), &task, previous_state, 160);

    assert_code(
        repository
            .save_implementation_transition(
                expected_version,
                &task,
                &duplicate_id_record,
                Some(&consent),
            )
            .expect_err("duplicate transition id must fail"),
        RepositoryErrorCode::OperationFailed,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before)
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent after rollback"),
        None,
        "consent insert must roll back with the rest of the transaction"
    );
}

#[test]
fn save_implementation_transition_rejects_version_mismatch_without_touching_consent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let before = task.clone();
    let stale_expected_version = before.version() + 41;

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: stale_expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 160,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 160);

    assert_code(
        repository
            .save_implementation_transition(stale_expected_version, &task, &record, Some(&consent))
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                consent.approved_task_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent"),
        None
    );
}

#[test]
fn planning_and_implementation_consents_for_the_same_task_and_version_are_independent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let planning_version = task.version();

    let planning_consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: planning_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 140,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    let planning_record = transition(TaskStateTransitionId::new(), &task, previous_state, 140);
    repository
        .save_planning_transition(
            planning_version,
            &task,
            &planning_record,
            Some(&planning_consent),
        )
        .expect("atomic planning transition");

    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    let implementation_version = task.version();

    // The Implementation consent for the same task and (new) version must be
    // recorded independently of the Planning consent above: neither the
    // repository nor the schema conflates them by (task_id, version) alone,
    // because work_kind is part of the composite key.
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                implementation_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read implementation consent before it exists"),
        None,
        "an Implementation consent must not be satisfied by a Planning consent"
    );

    let implementation_consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: implementation_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 160,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    let implementation_record =
        transition(TaskStateTransitionId::new(), &task, previous_state, 160);
    repository
        .save_implementation_transition(
            implementation_version,
            &task,
            &implementation_record,
            Some(&implementation_consent),
        )
        .expect("atomic implementation transition");

    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                planning_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("planning consent still readable")
            .expect("planning consent preserved"),
        planning_consent,
        "recording the Implementation consent must not disturb the Planning consent"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                implementation_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("implementation consent readable")
            .expect("implementation consent recorded"),
        implementation_consent
    );
}

#[test]
fn planning_and_implementation_consents_are_scoped_and_do_not_leak_across_data_scopes() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let planning_version = task.version();

    let planning_consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: planning_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 140,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    let planning_record = transition(TaskStateTransitionId::new(), &task, previous_state, 140);
    repository
        .save_planning_transition(
            planning_version,
            &task,
            &planning_record,
            Some(&planning_consent),
        )
        .expect("atomic planning transition");

    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                planning_version,
                ContextDataScope::ContextPackageV1,
            )
            .expect("read consent under the wrong scope"),
        None,
        "a LegacyPhase4 Planning consent must not satisfy a ContextPackageV1 lookup"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                planning_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent under the recorded scope")
            .expect("consent present under its recorded scope"),
        planning_consent
    );

    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    let implementation_version = task.version();
    let implementation_consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: implementation_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 160,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    let implementation_record =
        transition(TaskStateTransitionId::new(), &task, previous_state, 160);
    repository
        .save_implementation_transition(
            implementation_version,
            &task,
            &implementation_record,
            Some(&implementation_consent),
        )
        .expect("atomic implementation transition");

    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                implementation_version,
                ContextDataScope::ContextPackageV1,
            )
            .expect("read consent under the wrong scope"),
        None,
        "a LegacyPhase4 Implementation consent must not satisfy a ContextPackageV1 lookup"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                implementation_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent under the recorded scope")
            .expect("consent present under its recorded scope"),
        implementation_consent
    );
}

#[test]
fn review_consents_under_different_data_scopes_for_the_same_version_are_independent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let legacy = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            200,
        )
        .expect("record a LegacyPhase4 review consent");
    let context_package = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::ContextPackageV1,
            210,
        )
        .expect(
            "a ContextPackageV1 review consent for the same task/provider/work_kind/version \
             must be recorded independently of the LegacyPhase4 consent",
        );

    assert_ne!(legacy, context_package);
    assert_eq!(legacy.data_scope, ContextDataScope::LegacyPhase4);
    assert_eq!(
        context_package.data_scope,
        ContextDataScope::ContextPackageV1
    );

    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read LegacyPhase4 consent")
            .expect("LegacyPhase4 consent present"),
        legacy,
        "the ContextPackageV1 consent must not be returned for a LegacyPhase4 lookup"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .expect("read ContextPackageV1 consent")
            .expect("ContextPackageV1 consent present"),
        context_package,
        "the LegacyPhase4 consent must not be returned for a ContextPackageV1 lookup"
    );

    assert_eq!(
        repository.get_task(task.id()).expect("get_task"),
        Some(before),
        "recording a second, differently-scoped review consent must still never change task \
         state or version"
    );
}

#[test]
fn save_review_consent_persists_new_consent_without_touching_task_state_version_history_or_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let consent = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            200,
        )
        .expect("atomic review consent");

    assert_eq!(
        consent,
        ProviderConsent {
            task_id: task.id(),
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::LegacyPhase4,
            consented_at_ms: 200,
        }
    );
    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(before),
        "recording a Review consent must never change task state or version"
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history after"),
        history_before,
        "recording a Review consent must never add a transition history entry"
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "recording a Review consent must never change the ActiveTaskLease"
    );
    let stored = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
            ContextDataScope::LegacyPhase4,
        )
        .expect("read consent")
        .expect("consent persisted");
    assert_eq!(stored, consent);
}

#[test]
fn save_review_consent_reuses_an_existing_same_version_consent_without_inserting_a_duplicate() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();

    let first = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            200,
        )
        .expect("first call records a new consent");
    let second = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            999,
        )
        .expect("second call must reuse the existing consent");

    assert_eq!(
        second, first,
        "reusing an existing same-version consent must return it unchanged, not the new consented_at_ms"
    );
    assert_eq!(second.consented_at_ms, 200);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1
    );
}

#[test]
fn save_review_consent_rejects_non_reviewing_state_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    assert_code(
        repository
            .save_review_consent(
                expected_version,
                task.id(),
                ContextDataScope::LegacyPhase4,
                200,
            )
            .expect_err("Testing must not be accepted as Reviewing"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent"),
        None
    );
}

#[test]
fn save_review_consent_rejects_stale_expected_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let before = task.clone();
    let stale_expected_version = before.version() + 41;

    assert_code(
        repository
            .save_review_consent(
                stale_expected_version,
                task.id(),
                ContextDataScope::LegacyPhase4,
                200,
            )
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                stale_expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read consent"),
        None
    );
}

#[test]
fn review_consent_is_independent_of_planning_and_implementation_consents_at_earlier_versions() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);
    let planning_version = task.version();

    let planning_consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: planning_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 140,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning");
    let planning_record = transition(TaskStateTransitionId::new(), &task, previous_state, 140);
    repository
        .save_planning_transition(
            planning_version,
            &task,
            &planning_record,
            Some(&planning_consent),
        )
        .expect("atomic planning transition");

    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    let implementation_version = task.version();
    let implementation_consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: implementation_version,
        data_scope: ContextDataScope::LegacyPhase4,
        consented_at_ms: 160,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing");
    let implementation_record =
        transition(TaskStateTransitionId::new(), &task, previous_state, 160);
    repository
        .save_implementation_transition(
            implementation_version,
            &task,
            &implementation_record,
            Some(&implementation_consent),
        )
        .expect("atomic implementation transition");

    advance(&mut repository, &mut task, TaskState::Testing, 170);
    advance(&mut repository, &mut task, TaskState::Reviewing, 180);
    let review_version = task.version();

    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                review_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read review consent before it exists"),
        None,
        "a Review consent must not be satisfied by a Planning or Implementation consent"
    );

    let review_consent = repository
        .save_review_consent(
            review_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            300,
        )
        .expect("atomic review consent");

    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                planning_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("planning consent still readable")
            .expect("planning consent preserved"),
        planning_consent,
        "recording the Review consent must not disturb the Planning consent"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                implementation_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("implementation consent still readable")
            .expect("implementation consent preserved"),
        implementation_consent,
        "recording the Review consent must not disturb the Implementation consent"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                review_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("review consent readable")
            .expect("review consent recorded"),
        review_consent
    );
}

#[test]
fn save_planning_result_persists_completed_result_transition_and_history_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_planning(&mut repository, &mut task);
    let expected_version = task.version();
    let result = completed_planning_result(task.id(), "the masked plan");

    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 150);

    repository
        .save_planning_result(expected_version, &task, &record, &result, false)
        .expect("atomic planning result");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "AwaitingDesignApproval is not terminal and must keep the lease"
    );
    let raw = fixture.database.open_raw();
    let stored: (String, String, String, i64, i64, i64, i64, String) = raw
        .query_row(
            "SELECT provider, work_kind, outcome, exit_code, turn_count,
                    started_at_ms, completed_at_ms, plan_text
             FROM task_planning_results WHERE task_id = ?1",
            [task.id().to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .expect("read stored planning result");
    assert_eq!(stored.0, "Claude");
    assert_eq!(stored.1, "Planning");
    assert_eq!(stored.2, "Completed");
    assert_eq!(stored.3, 0);
    assert_eq!(stored.4, 5);
    assert_eq!(stored.5, 135);
    assert_eq!(stored.6, 150);
    assert_eq!(stored.7, "the masked plan");

    let loaded = repository
        .get_task_planning_result(task.id())
        .expect("read back planning result")
        .expect("a result row exists");
    assert_eq!(loaded, result);
}

#[test]
fn get_task_planning_result_returns_none_when_no_attempt_has_been_recorded() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);

    assert_eq!(
        repository
            .get_task_planning_result(task.id())
            .expect("lookup succeeds"),
        None
    );
}

#[test]
fn save_planning_result_terminal_outcome_releases_the_active_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_planning(&mut repository, &mut task);
    let expected_version = task.version();
    let mut result = completed_planning_result(task.id(), "unused");
    result.outcome = PlanningResultOutcome::Failed;
    result.plan_text = None;

    let previous_state = task.state();
    task.transition_to(TaskState::Failed, 150)
        .expect("Planning -> Failed");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 150);

    repository
        .save_planning_result(expected_version, &task, &record, &result, true)
        .expect("atomic terminal planning result");

    assert_eq!(
        repository.active_lease().expect("lease query"),
        None,
        "a terminal outcome must release the active lease"
    );
    assert_eq!(
        repository
            .get_task(task.id())
            .expect("get task")
            .expect("task exists")
            .state(),
        TaskState::Failed
    );
}

fn cancelled_planning_result(task_id: TaskId) -> TaskPlanningResultRecord {
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
    }
}

#[test]
fn save_planning_result_cancelled_outcome_transitions_and_releases_the_active_lease_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_planning(&mut repository, &mut task);
    let expected_version = task.version();
    let result = cancelled_planning_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::Cancelled, 150)
        .expect("Planning -> Cancelled");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 150);

    repository
        .save_planning_result(expected_version, &task, &record, &result, true)
        .expect("atomic confirmed-cancellation planning result");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone()),
        "the task's Cancelled state must be persisted"
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record,
        "the Planning -> Cancelled transition must be recorded in history"
    );
    assert_eq!(
        repository.active_lease().expect("lease query"),
        None,
        "a confirmed cancellation must release the active lease in the same transaction"
    );
    let raw = fixture.database.open_raw();
    let stored_outcome: String = raw
        .query_row(
            "SELECT outcome FROM task_planning_results WHERE task_id = ?1",
            [task.id().to_string()],
            |row| row.get(0),
        )
        .expect("read stored planning result");
    assert_eq!(stored_outcome, "Cancelled");
}

#[test]
fn save_planning_result_rolls_back_everything_when_history_insert_fails() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    advance_to_planning(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    let result = completed_planning_result(task.id(), "should not survive rollback");

    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval");
    // Reusing an already-persisted transition id forces the final history
    // insert to fail on its primary key after the planning-result row has
    // already been written inside the same transaction.
    let duplicate_id_record = transition(initial.id(), &task, previous_state, 150);

    assert_code(
        repository
            .save_planning_result(
                expected_version,
                &task,
                &duplicate_id_record,
                &result,
                false,
            )
            .expect_err("duplicate transition id must fail"),
        RepositoryErrorCode::OperationFailed,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_planning_results"),
        0
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease after rollback")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "a rolled-back terminal-shaped write must not have released the lease"
    );
}

#[test]
fn save_planning_result_rejects_version_mismatch_without_writing_a_result_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_planning(&mut repository, &mut task);
    let before = task.clone();
    let stale_expected_version = before.version() + 41;
    let result = completed_planning_result(task.id(), "must not be persisted");

    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 150);

    assert_code(
        repository
            .save_planning_result(stale_expected_version, &task, &record, &result, false)
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_planning_results"),
        0
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
        TaskState::Planning,
        TaskState::Implementing,
        TaskState::Testing,
        TaskState::Reviewing,
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

    advance(&mut repository, &mut task, TaskState::Planning, 160);
    advance(&mut repository, &mut task, TaskState::RecoveryRequired, 170);
    assert_eq!(task.resume_target_state(), None);
    task.set_recovery_target(
        TaskState::Planning,
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
        Some(TaskState::Planning)
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
    assert_eq!(task.resume_target_state(), Some(TaskState::Planning));

    let expected = task.version();
    task.resume_from_pause(
        TaskState::Planning,
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

fn completed_implementation_result(task_id: TaskId) -> TaskImplementationResultRecord {
    TaskImplementationResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        outcome: ImplementationResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(7),
        started_at_ms: 155,
        completed_at_ms: 170,
    }
}

fn cancelled_implementation_result(task_id: TaskId) -> TaskImplementationResultRecord {
    TaskImplementationResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        outcome: ImplementationResultOutcome::Cancelled,
        exit_code: None,
        turn_count: None,
        started_at_ms: 155,
        completed_at_ms: 170,
    }
}

fn recovery_required_implementation_result(task_id: TaskId) -> TaskImplementationResultRecord {
    TaskImplementationResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        outcome: ImplementationResultOutcome::RecoveryRequired,
        exit_code: Some(1),
        turn_count: None,
        started_at_ms: 155,
        completed_at_ms: 170,
    }
}

#[test]
fn save_implementation_result_completed_transitions_to_testing_and_keeps_the_lease_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_implementing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = completed_implementation_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::Testing, 170)
        .expect("Implementing -> Testing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 170);

    repository
        .save_implementation_result(expected_version, &task, &record, &result)
        .expect("atomic implementation result");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Testing is not terminal and must keep the lease"
    );
    let raw = fixture.database.open_raw();
    let stored: (String, String, String, i64, i64, i64, i64) = raw
        .query_row(
            "SELECT provider, work_kind, outcome, exit_code, turn_count,
                    started_at_ms, completed_at_ms
             FROM task_implementation_results WHERE task_id = ?1",
            [task.id().to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .expect("read stored implementation result");
    assert_eq!(stored.0, "Claude");
    assert_eq!(stored.1, "Implementation");
    assert_eq!(stored.2, "Completed");
    assert_eq!(stored.3, 0);
    assert_eq!(stored.4, 7);
    assert_eq!(stored.5, 155);
    assert_eq!(stored.6, 170);

    let loaded = repository
        .get_task_implementation_result(task.id())
        .expect("read back implementation result")
        .expect("a result row exists");
    assert_eq!(loaded, result);
}

#[test]
fn get_task_implementation_result_returns_none_when_no_attempt_has_been_recorded() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);

    assert_eq!(
        repository
            .get_task_implementation_result(task.id())
            .expect("lookup succeeds"),
        None
    );
}

#[test]
fn save_implementation_result_confirmed_cancellation_pauses_with_implementing_resume_target_and_keeps_the_lease()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_implementing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = cancelled_implementation_result(task.id());

    let previous_state = task.state();
    task.pause(170).expect("Implementing -> Paused");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 170);

    repository
        .save_implementation_result(expected_version, &task, &record, &result)
        .expect("atomic confirmed-cancellation implementation result");

    let reloaded = repository
        .get_task(task.id())
        .expect("get task")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Paused);
    assert_eq!(
        reloaded.resume_target_state(),
        Some(TaskState::Implementing),
        "a confirmed cancellation must be resumable back to Implementing"
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Paused is not terminal and must keep the lease"
    );
    let stored_outcome: String = fixture
        .database
        .open_raw()
        .query_row(
            "SELECT outcome FROM task_implementation_results WHERE task_id = ?1",
            [task.id().to_string()],
            |row| row.get(0),
        )
        .expect("read stored implementation result");
    assert_eq!(stored_outcome, "Cancelled");
}

#[test]
fn save_implementation_result_recovery_required_keeps_the_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_implementing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = recovery_required_implementation_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::RecoveryRequired, 170)
        .expect("Implementing -> RecoveryRequired");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 170);

    repository
        .save_implementation_result(expected_version, &task, &record, &result)
        .expect("atomic recovery-required implementation result");

    assert_eq!(
        repository
            .get_task(task.id())
            .expect("get task")
            .expect("task exists")
            .state(),
        TaskState::RecoveryRequired
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn save_implementation_result_rolls_back_everything_when_history_insert_fails() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    advance_to_implementing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    let result = completed_implementation_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::Testing, 170)
        .expect("Implementing -> Testing");
    // Reusing an already-persisted transition id forces the final history
    // insert to fail on its primary key after the implementation-result row
    // has already been written inside the same transaction.
    let duplicate_id_record = transition(initial.id(), &task, previous_state, 170);

    assert_code(
        repository
            .save_implementation_result(expected_version, &task, &duplicate_id_record, &result)
            .expect_err("duplicate transition id must fail"),
        RepositoryErrorCode::OperationFailed,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_implementation_results"),
        0
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease after rollback")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "a rolled-back write must not have released the lease"
    );
}

#[test]
fn save_implementation_result_rejects_version_mismatch_without_writing_a_result_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_implementing(&mut repository, &mut task);
    let before = task.clone();
    let stale_expected_version = before.version() + 41;
    let result = completed_implementation_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::Testing, 170)
        .expect("Implementing -> Testing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 170);

    assert_code(
        repository
            .save_implementation_result(stale_expected_version, &task, &record, &result)
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_implementation_results"),
        0
    );
}

#[test]
fn save_implementation_result_rejects_a_duplicate_result_row_for_the_same_task() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_implementing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    fixture
        .database
        .open_raw()
        .execute(
            "INSERT INTO task_implementation_results (
                task_id, provider, work_kind, outcome, exit_code, turn_count,
                started_at_ms, completed_at_ms
             ) VALUES (?1, 'Claude', 'Implementation', 'Completed', 0, 1, 100, 110)",
            [task.id().to_string()],
        )
        .expect("seed a pre-existing result row for this task");

    let result = completed_implementation_result(task.id());
    let previous_state = task.state();
    task.transition_to(TaskState::Testing, 170)
        .expect("Implementing -> Testing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 170);

    assert_code(
        repository
            .save_implementation_result(expected_version, &task, &record, &result)
            .expect_err("task_id is 1:1: a second result row for the same task must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before),
        "the whole write must roll back, including the state transition"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_implementation_results"),
        1
    );
}

fn completed_review_result(task_id: TaskId, review_text: &str) -> TaskReviewResultRecord {
    TaskReviewResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        outcome: ReviewResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(4),
        started_at_ms: 175,
        completed_at_ms: 190,
        review_text: Some(review_text.to_owned()),
    }
}

fn failed_review_result(task_id: TaskId) -> TaskReviewResultRecord {
    TaskReviewResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        outcome: ReviewResultOutcome::Failed,
        exit_code: Some(1),
        turn_count: None,
        started_at_ms: 175,
        completed_at_ms: 190,
        review_text: None,
    }
}

fn cancelled_review_result(task_id: TaskId) -> TaskReviewResultRecord {
    TaskReviewResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        outcome: ReviewResultOutcome::Cancelled,
        exit_code: None,
        turn_count: None,
        started_at_ms: 175,
        completed_at_ms: 190,
        review_text: None,
    }
}

fn recovery_required_review_result(task_id: TaskId) -> TaskReviewResultRecord {
    TaskReviewResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        outcome: ReviewResultOutcome::RecoveryRequired,
        exit_code: None,
        turn_count: None,
        started_at_ms: 175,
        completed_at_ms: 190,
        review_text: None,
    }
}

#[test]
fn save_review_result_persists_completed_result_transition_and_history_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = completed_review_result(task.id(), "the masked review");

    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingUserDiffApproval, 190)
        .expect("Reviewing -> AwaitingUserDiffApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 190);

    repository
        .save_review_result(expected_version, &task, &record, &result, false)
        .expect("atomic review result");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "AwaitingUserDiffApproval is not terminal and must keep the lease"
    );
    let raw = fixture.database.open_raw();
    let stored: (String, String, String, i64, i64, i64, i64, String) = raw
        .query_row(
            "SELECT provider, work_kind, outcome, exit_code, turn_count,
                    started_at_ms, completed_at_ms, review_text
             FROM task_review_results WHERE task_id = ?1",
            [task.id().to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .expect("read stored review result");
    assert_eq!(stored.0, "Claude");
    assert_eq!(stored.1, "Review");
    assert_eq!(stored.2, "Completed");
    assert_eq!(stored.3, 0);
    assert_eq!(stored.4, 4);
    assert_eq!(stored.5, 175);
    assert_eq!(stored.6, 190);
    assert_eq!(stored.7, "the masked review");

    let loaded = repository
        .get_task_review_result(task.id())
        .expect("read back review result")
        .expect("a result row exists");
    assert_eq!(loaded, result);
}

#[test]
fn get_task_review_result_returns_none_when_no_attempt_has_been_recorded() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);

    assert_eq!(
        repository
            .get_task_review_result(task.id())
            .expect("lookup succeeds"),
        None
    );
}

#[test]
fn save_review_result_failed_outcome_releases_the_active_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = failed_review_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::Failed, 190)
        .expect("Reviewing -> Failed");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 190);

    repository
        .save_review_result(expected_version, &task, &record, &result, true)
        .expect("atomic terminal review result");

    assert_eq!(
        repository.active_lease().expect("lease query"),
        None,
        "a terminal Failed outcome must release the active lease"
    );
    assert_eq!(
        repository
            .get_task(task.id())
            .expect("get task")
            .expect("task exists")
            .state(),
        TaskState::Failed
    );
}

#[test]
fn save_review_result_confirmed_cancellation_pauses_with_reviewing_resume_target_and_keeps_the_lease()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = cancelled_review_result(task.id());

    let previous_state = task.state();
    task.pause(190).expect("Reviewing -> Paused");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 190);

    repository
        .save_review_result(expected_version, &task, &record, &result, false)
        .expect("atomic confirmed-cancellation review result");

    let reloaded = repository
        .get_task(task.id())
        .expect("get task")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Paused);
    assert_eq!(
        reloaded.resume_target_state(),
        Some(TaskState::Reviewing),
        "a confirmed cancellation must be resumable back to Reviewing"
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Paused is not terminal and must keep the lease"
    );
    let stored_outcome: String = fixture
        .database
        .open_raw()
        .query_row(
            "SELECT outcome FROM task_review_results WHERE task_id = ?1",
            [task.id().to_string()],
            |row| row.get(0),
        )
        .expect("read stored review result");
    assert_eq!(stored_outcome, "Cancelled");
}

#[test]
fn save_review_result_recovery_required_keeps_the_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let result = recovery_required_review_result(task.id());

    let previous_state = task.state();
    task.transition_to(TaskState::RecoveryRequired, 190)
        .expect("Reviewing -> RecoveryRequired");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 190);

    repository
        .save_review_result(expected_version, &task, &record, &result, false)
        .expect("atomic recovery-required review result");

    assert_eq!(
        repository
            .get_task(task.id())
            .expect("get task")
            .expect("task exists")
            .state(),
        TaskState::RecoveryRequired
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn save_review_result_rolls_back_everything_when_history_insert_fails() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    let result = completed_review_result(task.id(), "should not survive rollback");

    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingUserDiffApproval, 190)
        .expect("Reviewing -> AwaitingUserDiffApproval");
    // Reusing an already-persisted transition id forces the final history
    // insert to fail on its primary key after the review-result row has
    // already been written inside the same transaction.
    let duplicate_id_record = transition(initial.id(), &task, previous_state, 190);

    assert_code(
        repository
            .save_review_result(
                expected_version,
                &task,
                &duplicate_id_record,
                &result,
                false,
            )
            .expect_err("duplicate transition id must fail"),
        RepositoryErrorCode::OperationFailed,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_review_results"),
        0
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease after rollback")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "a rolled-back write must not have released the lease"
    );
}

#[test]
fn save_review_result_rejects_version_mismatch_without_writing_a_result_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let before = task.clone();
    let stale_expected_version = before.version() + 41;
    let result = completed_review_result(task.id(), "must not be persisted");

    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingUserDiffApproval, 190)
        .expect("Reviewing -> AwaitingUserDiffApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 190);

    assert_code(
        repository
            .save_review_result(stale_expected_version, &task, &record, &result, false)
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_review_results"),
        0
    );
}

#[test]
fn save_review_result_rejects_a_duplicate_result_row_for_the_same_task() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    fixture
        .database
        .open_raw()
        .execute(
            "INSERT INTO task_review_results (
                task_id, provider, work_kind, outcome, exit_code, turn_count,
                started_at_ms, completed_at_ms, review_text
             ) VALUES (?1, 'Claude', 'Review', 'Completed', 0, 1, 100, 110, 'earlier review')",
            [task.id().to_string()],
        )
        .expect("seed a pre-existing result row for this task");

    let result = completed_review_result(task.id(), "the new review");
    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingUserDiffApproval, 190)
        .expect("Reviewing -> AwaitingUserDiffApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 190);

    assert_code(
        repository
            .save_review_result(expected_version, &task, &record, &result, false)
            .expect_err("task_id is 1:1: a second result row for the same task must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before),
        "the whole write must roll back, including the state transition"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_review_results"),
        1
    );
}

fn validation_command_approval(
    task_id: TaskId,
    approved_task_version: u64,
    kind: ValidationCommandKind,
    executable: &str,
    arguments: &[&str],
) -> ValidationCommandApprovalRecord {
    ValidationCommandApprovalRecord {
        task_id,
        approved_task_version,
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
        kind,
        executable: executable.to_owned(),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        approved_executable_path: "C:/tools/cargo/bin/cargo.exe".to_owned(),
        executable_volume_serial_hex: "0000000000000002".to_owned(),
        executable_file_id_hex: "00000000000000000000000000000002".to_owned(),
        tool_directory_path: "C:/tools/cargo/bin".to_owned(),
        tool_directory_volume_serial_hex: "0000000000000001".to_owned(),
        tool_directory_file_id_hex: "00000000000000000000000000000001".to_owned(),
        approved_cargo_home_path: None,
        cargo_home_volume_serial_hex: None,
        cargo_home_file_id_hex: None,
        approved_rustup_home_path: None,
        rustup_home_volume_serial_hex: None,
        rustup_home_file_id_hex: None,
        target_project_id: None,
        target_project_identity_revision: None,
        target_root_volume_serial_hex: None,
        target_root_file_id_hex: None,
        approved_at_ms: 175,
    }
}

#[test]
fn save_validation_command_approval_persists_and_lists_back_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test", "--workspace"],
    );

    repository
        .save_validation_command_approval(&approval)
        .expect("atomic validation command approval");

    let stored = repository
        .list_validation_command_approvals(task.id(), task.version())
        .expect("read back validation command approvals");
    assert_eq!(stored, vec![approval]);
}

#[test]
fn project_root_approvals_are_scope_separated_and_require_awaiting_user_diff_approval() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_user_diff_approval(&mut repository, &mut task);

    let test_approval =
        project_root_validation_approval(&mut repository, &task, ValidationCommandKind::Test);
    repository
        .save_validation_command_approval(&test_approval)
        .expect("save ProjectRoot approval");

    assert!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect("list TaskWorktree approvals")
            .is_empty()
    );
    let project_root = repository
        .list_validation_command_approvals_for_scope(
            task.id(),
            task.version(),
            ValidationExecutionScope::ProjectRoot,
        )
        .expect("list ProjectRoot approvals");
    assert_eq!(project_root, vec![test_approval.clone()]);

    let mut mismatched =
        project_root_validation_approval(&mut repository, &task, ValidationCommandKind::Build);
    mismatched.target_project_identity_revision = Some(99);
    assert_code(
        repository
            .save_validation_command_approval(&mismatched)
            .expect_err("mismatched project identity snapshot must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    advance(&mut repository, &mut task, TaskState::Merging, 200);
    let wrong_state =
        project_root_validation_approval(&mut repository, &task, ValidationCommandKind::Build);
    assert_code(
        repository
            .save_validation_command_approval(&wrong_state)
            .expect_err("Merging cannot create a new ProjectRoot approval"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

fn prepare_post_merge_task(
    repository: &mut impl FoundationRepository,
    project_id: ProjectId,
) -> (Task, u64) {
    let (mut task, _) = create_task(repository, project_id);
    advance_to_awaiting_user_diff_approval(repository, &mut task);
    let approval_version = task.version();
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        let approval = project_root_validation_approval(repository, &task, kind);
        repository
            .save_validation_command_approval(&approval)
            .expect("save required ProjectRoot approval");
    }
    advance(repository, &mut task, TaskState::Merging, 200);
    advance(repository, &mut task, TaskState::PostMergeTesting, 210);
    (task, approval_version)
}

fn post_merge_attempt(
    task: &Task,
    approval_task_version: u64,
    outcome: PostMergeValidationResultOutcome,
) -> PostMergeValidationResultAttempt {
    PostMergeValidationResultAttempt {
        task_id: task.id(),
        approval_task_version,
        post_merge_task_version: task.version(),
        execution_scope: ValidationExecutionScope::ProjectRoot,
        kind: ValidationCommandKind::Build,
        outcome,
        exit_code: match outcome {
            PostMergeValidationResultOutcome::Success => Some(0),
            PostMergeValidationResultOutcome::ExitFailure => Some(1),
            _ => None,
        },
        safe_summary: "post-merge validation result".to_owned(),
        started_at_ms: 220,
        completed_at_ms: 230,
    }
}

#[test]
fn post_merge_final_success_is_atomic_with_completed_history_and_lease_release() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, approval_version) = prepare_post_merge_task(&mut repository, fixture.project_id);
    let expected_version = task.version();
    let mut intermediate = post_merge_attempt(
        &task,
        approval_version,
        PostMergeValidationResultOutcome::Success,
    );
    intermediate.kind = ValidationCommandKind::Test;
    repository
        .append_post_merge_validation_result(&intermediate)
        .expect("append intermediate success without a state transition");
    assert_eq!(
        repository
            .get_task(task.id())
            .expect("read task")
            .expect("task")
            .state(),
        TaskState::PostMergeTesting
    );
    let source_task = task.clone();
    let from_state = task.state();
    task.transition_to(TaskState::Completed, 240)
        .expect("PostMergeTesting -> Completed");
    let record = transition(TaskStateTransitionId::new(), &task, from_state, 240);
    let attempt = post_merge_attempt(
        &source_task,
        approval_version,
        PostMergeValidationResultOutcome::Success,
    );

    repository
        .finalize_post_merge_validation_batch(expected_version, &task, &record, &attempt)
        .expect("atomic post-merge success finalize");

    assert_eq!(
        repository
            .get_task(task.id())
            .expect("read task")
            .expect("task")
            .state(),
        TaskState::Completed
    );
    assert!(repository.active_lease().expect("read lease").is_none());
    assert_eq!(
        repository
            .list_post_merge_validation_results(
                task.id(),
                approval_version,
                expected_version,
                ValidationCommandKind::Build,
            )
            .expect("list result")
            .len(),
        1
    );
}

#[test]
fn post_merge_non_success_results_atomically_require_recovery_and_keep_the_lease() {
    for outcome in [
        PostMergeValidationResultOutcome::ExitFailure,
        PostMergeValidationResultOutcome::TimedOut,
        PostMergeValidationResultOutcome::StdoutBoundExceeded,
        PostMergeValidationResultOutcome::BindingRejected,
        PostMergeValidationResultOutcome::Cancelled,
        PostMergeValidationResultOutcome::Uncertain,
    ] {
        let fixture = Fixture::new();
        let mut connection = fixture.open();
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        let (mut task, approval_version) =
            prepare_post_merge_task(&mut repository, fixture.project_id);
        let source_task = task.clone();
        let expected_version = task.version();
        let from_state = task.state();
        task.transition_to(TaskState::RecoveryRequired, 240)
            .expect("PostMergeTesting -> RecoveryRequired");
        let record = transition(TaskStateTransitionId::new(), &task, from_state, 240);
        let attempt = post_merge_attempt(&source_task, approval_version, outcome);

        repository
            .finalize_post_merge_validation_batch(expected_version, &task, &record, &attempt)
            .expect("atomic fail-closed finalize");

        assert_eq!(
            repository
                .get_task(task.id())
                .expect("read task")
                .expect("task")
                .state(),
            TaskState::RecoveryRequired
        );
        assert_eq!(
            repository
                .active_lease()
                .expect("read lease")
                .map(|lease| lease.task_id),
            Some(task.id())
        );
    }
}

#[test]
fn save_validation_command_approval_rejects_a_task_that_is_not_implementing_or_testing() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test"],
    );

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("AwaitingDesignApproval is not a valid state for this approval"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect("list approvals"),
        Vec::new()
    );
}

#[test]
fn save_validation_command_approval_rejects_a_version_mismatch() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let approval = validation_command_approval(
        task.id(),
        task.version() + 1,
        ValidationCommandKind::Test,
        "cargo",
        &["test"],
    );

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("a version other than the task's current version must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn save_validation_command_approval_rejects_a_duplicate_for_the_same_kind() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let first = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test"],
    );
    repository
        .save_validation_command_approval(&first)
        .expect("first approval succeeds");

    let second = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test", "--workspace"],
    );
    assert_code(
        repository
            .save_validation_command_approval(&second)
            .expect_err("a second approval for the same (task, version, kind) must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect("list approvals"),
        vec![first]
    );
}

#[test]
fn save_validation_command_approval_rejects_arguments_that_escape_the_worktree() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test", "../../../outside-the-worktree"],
    );

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("a path-traversal argument must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect("list approvals"),
        Vec::new()
    );
}

#[test]
fn save_validation_command_approval_rejects_an_absolute_executable_path() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "C:/Windows/System32/cmd.exe",
        &["test"],
    );

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("an absolute executable path must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn save_validation_command_approval_rejects_a_relative_approved_executable_path() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let mut approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test"],
    );
    approval.approved_executable_path = "cargo.exe".to_owned();

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("a relative approved_executable_path must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn save_validation_command_approval_rejects_path_traversal_in_the_tool_directory_path() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let mut approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test"],
    );
    approval.tool_directory_path = "C:/tools/cargo/bin/../../escape".to_owned();

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("a `..` component in the tool directory path must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn save_validation_command_approval_rejects_malformed_stable_identity_hex_values() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);

    for corrupt in [
        |approval: &mut ValidationCommandApprovalRecord| {
            approval.executable_volume_serial_hex = "not-hex-at-all!!".to_owned();
        },
        |approval: &mut ValidationCommandApprovalRecord| {
            approval.executable_file_id_hex = "00".to_owned();
        },
        |approval: &mut ValidationCommandApprovalRecord| {
            // Uppercase hex must be rejected: the persisted format is
            // lowercase-only, matching the migration's `NOT GLOB
            // '*[^0-9a-f]*'` check exactly.
            approval.tool_directory_volume_serial_hex = "00000000000000AB".to_owned();
        },
    ] {
        let mut approval = validation_command_approval(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            "cargo",
            &["test"],
        );
        corrupt(&mut approval);

        assert_code(
            repository
                .save_validation_command_approval(&approval)
                .expect_err("a malformed stable-identity hex value must be rejected"),
            RepositoryErrorCode::InvalidAggregate,
        );
    }
    assert_eq!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect("list approvals"),
        Vec::new(),
        "none of the malformed attempts may have persisted"
    );
}

#[test]
fn save_validation_command_approval_persists_and_lists_back_an_approved_cargo_and_rustup_home_binding()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let mut approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test", "--workspace"],
    );
    approval.approved_cargo_home_path = Some("C:/tools/cargo-home".to_owned());
    approval.cargo_home_volume_serial_hex = Some("0000000000000003".to_owned());
    approval.cargo_home_file_id_hex = Some("00000000000000000000000000000003".to_owned());
    approval.approved_rustup_home_path = Some("C:/tools/rustup-home".to_owned());
    approval.rustup_home_volume_serial_hex = Some("0000000000000004".to_owned());
    approval.rustup_home_file_id_hex = Some("00000000000000000000000000000004".to_owned());

    repository
        .save_validation_command_approval(&approval)
        .expect("an approved cargo/rustup home binding is stored atomically");

    let stored = repository
        .list_validation_command_approvals(task.id(), task.version())
        .expect("read back validation command approvals");
    assert_eq!(stored, vec![approval]);
}

#[test]
fn save_validation_command_approval_rejects_a_partially_populated_environment_binding() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);

    for corrupt in [
        |approval: &mut ValidationCommandApprovalRecord| {
            approval.approved_cargo_home_path = Some("C:/tools/cargo-home".to_owned());
        },
        |approval: &mut ValidationCommandApprovalRecord| {
            approval.cargo_home_volume_serial_hex = Some("0000000000000003".to_owned());
        },
        |approval: &mut ValidationCommandApprovalRecord| {
            approval.approved_rustup_home_path = Some("C:/tools/rustup-home".to_owned());
        },
    ] {
        let mut approval = validation_command_approval(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            "cargo",
            &["test"],
        );
        corrupt(&mut approval);

        assert_code(
            repository
                .save_validation_command_approval(&approval)
                .expect_err(
                    "a home binding with only some of its three fields set must be rejected",
                ),
            RepositoryErrorCode::InvalidAggregate,
        );
    }
    assert_eq!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect("list approvals"),
        Vec::new(),
        "none of the partially-populated attempts may have persisted"
    );
}

#[test]
fn save_validation_command_approval_rejects_a_relative_or_malformed_environment_home_path() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let mut approval = validation_command_approval(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        "cargo",
        &["test"],
    );
    approval.approved_cargo_home_path = Some("cargo-home".to_owned());
    approval.cargo_home_volume_serial_hex = Some("0000000000000003".to_owned());
    approval.cargo_home_file_id_hex = Some("00000000000000000000000000000003".to_owned());

    assert_code(
        repository
            .save_validation_command_approval(&approval)
            .expect_err("a relative CARGO_HOME path must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

fn validation_command_result_attempt(
    task_id: TaskId,
    approved_task_version: u64,
    kind: ValidationCommandKind,
    outcome: ValidationCommandResultOutcome,
    exit_code: Option<i32>,
    safe_summary: &str,
) -> ValidationCommandResultAttempt {
    ValidationCommandResultAttempt {
        task_id,
        approved_task_version,
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
        kind,
        outcome,
        exit_code,
        safe_summary: safe_summary.to_owned(),
        started_at_ms: 100,
        completed_at_ms: 200,
    }
}

#[test]
fn append_validation_command_result_computes_sequence_starting_at_one_and_increments() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval to append results against");

    let first = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "cargo test passed",
        ))
        .expect("the first attempt is appended");
    assert_eq!(first.attempt_sequence, 1);
    assert_eq!(first.outcome, ValidationCommandResultOutcome::Success);
    assert_eq!(first.exit_code, Some(0));
    assert_eq!(first.safe_summary, "cargo test passed");

    let second = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::ExitFailure,
            Some(101),
            "cargo test failed",
        ))
        .expect("the second attempt is appended");
    assert_eq!(second.attempt_sequence, 2);

    let third = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "cargo test passed again",
        ))
        .expect("the third attempt is appended");
    assert_eq!(third.attempt_sequence, 3);

    let stored = repository
        .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
        .expect("list results");
    assert_eq!(stored, vec![first, second, third]);
}

#[test]
fn append_validation_command_result_is_independent_per_kind_and_task_version() {
    // Only one task may be active at a time (the singleton `ActiveTaskLease`
    // invariant), so version independence is exercised on a single task
    // that re-enters Testing via the existing `Testing -> AutoFixing ->
    // Testing` cycle rather than via a second concurrent task.
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let first_version = task.version();
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            first_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed a Test approval at the first version");
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            first_version,
            ValidationCommandKind::Build,
            "cargo",
            &["build", "--workspace"],
        ))
        .expect("seed a Build approval at the same first version");

    advance(&mut repository, &mut task, TaskState::AutoFixing, 180);
    advance(&mut repository, &mut task, TaskState::Testing, 190);
    let second_version = task.version();
    assert_ne!(
        first_version, second_version,
        "the AutoFixing round trip must bump the task version"
    );
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            second_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed a fresh Test approval at the second version");

    let first_version_test_attempt = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            first_version,
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "first version, test kind",
        ))
        .expect("Test kind attempt at the first version");
    let first_version_build_attempt = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            first_version,
            ValidationCommandKind::Build,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "first version, build kind",
        ))
        .expect("Build kind attempt at the first version starts its own sequence");
    let second_version_test_attempt = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            second_version,
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "second version, test kind",
        ))
        .expect("the second version's own approval starts its own sequence");

    assert_eq!(first_version_test_attempt.attempt_sequence, 1);
    assert_eq!(first_version_build_attempt.attempt_sequence, 1);
    assert_eq!(second_version_test_attempt.attempt_sequence, 1);

    assert_eq!(
        repository
            .list_validation_command_results(task.id(), first_version, ValidationCommandKind::Test)
            .expect("list first-version Test results"),
        vec![first_version_test_attempt]
    );
    assert_eq!(
        repository
            .list_validation_command_results(task.id(), first_version, ValidationCommandKind::Build)
            .expect("list first-version Build results"),
        vec![first_version_build_attempt]
    );
    assert_eq!(
        repository
            .list_validation_command_results(task.id(), second_version, ValidationCommandKind::Test)
            .expect("list second-version Test results"),
        vec![second_version_test_attempt]
    );
}

#[test]
fn append_validation_command_result_rejects_when_no_matching_approval_exists() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);

    let error = repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "ok",
        ))
        .expect_err("no Test approval was ever recorded for this task/version");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);
    assert_eq!(
        repository
            .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
            .expect("list results"),
        Vec::new()
    );
}

#[test]
fn append_validation_command_result_rejects_backwards_timestamps() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");
    let mut attempt = validation_command_result_attempt(
        task.id(),
        task.version(),
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Success,
        Some(0),
        "ok",
    );
    attempt.started_at_ms = 200;
    attempt.completed_at_ms = 100;

    assert_code(
        repository
            .append_validation_command_result(&attempt)
            .expect_err("completed_at_ms before started_at_ms must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );
}

#[test]
fn append_validation_command_result_rejects_empty_or_oversized_safe_summary() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");

    for summary in ["", &"x".repeat(2001)] {
        let attempt = validation_command_result_attempt(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::Success,
            Some(0),
            summary,
        );
        assert_code(
            repository
                .append_validation_command_result(&attempt)
                .expect_err("an empty or oversized safe_summary must be rejected"),
            RepositoryErrorCode::InvalidAggregate,
        );
    }
    assert_eq!(
        repository
            .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
            .expect("list results"),
        Vec::new(),
        "none of the rejected attempts may have persisted"
    );
}

#[test]
fn append_validation_command_result_rejects_an_exit_code_outcome_mismatch() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");

    for (outcome, exit_code) in [
        (ValidationCommandResultOutcome::Success, None),
        (ValidationCommandResultOutcome::ExitFailure, None),
        (ValidationCommandResultOutcome::TimedOut, Some(0)),
        (ValidationCommandResultOutcome::StdoutBoundExceeded, Some(1)),
        (ValidationCommandResultOutcome::Cancelled, Some(0)),
        (ValidationCommandResultOutcome::Uncertain, Some(0)),
    ] {
        let attempt = validation_command_result_attempt(
            task.id(),
            task.version(),
            ValidationCommandKind::Test,
            outcome,
            exit_code,
            "ok",
        );
        assert_code(
            repository
                .append_validation_command_result(&attempt)
                .expect_err("exit_code must be present only for a confirmed Success/ExitFailure"),
            RepositoryErrorCode::InvalidAggregate,
        );
    }
    assert_eq!(
        repository
            .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
            .expect("list results"),
        Vec::new(),
        "none of the rejected attempts may have persisted"
    );
}

#[test]
fn list_validation_command_results_returns_empty_when_nothing_appended() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);

    assert_eq!(
        repository
            .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
            .expect("list results"),
        Vec::new()
    );
}

#[test]
fn finalize_validation_command_batch_success_transitions_to_reviewing_and_keeps_the_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Success,
        Some(0),
        "cargo test passed",
    );

    let previous_state = task.state();
    task.transition_to(TaskState::Reviewing, 200)
        .expect("Testing -> Reviewing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 200);

    repository
        .finalize_validation_command_batch(expected_version, &task, &record, &attempt)
        .expect("atomic testing batch finalize");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history")
            .last()
            .expect("latest transition"),
        &record
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Reviewing is not terminal and must keep the lease"
    );
    let stored = repository
        .list_validation_command_results(task.id(), expected_version, ValidationCommandKind::Test)
        .expect("read back result");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].attempt_sequence, 1);
    assert_eq!(stored[0].outcome, ValidationCommandResultOutcome::Success);
}

#[test]
fn finalize_validation_command_batch_confirmed_cancellation_pauses_with_testing_resume_target_and_keeps_the_lease()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Cancelled,
        None,
        "validation command was cancelled",
    );

    let previous_state = task.state();
    task.pause(200).expect("Testing -> Paused");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 200);

    repository
        .finalize_validation_command_batch(expected_version, &task, &record, &attempt)
        .expect("atomic confirmed-cancellation testing batch finalize");

    let reloaded = repository
        .get_task(task.id())
        .expect("get task")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Paused);
    assert_eq!(
        reloaded.resume_target_state(),
        Some(TaskState::Testing),
        "a confirmed cancellation must be resumable back to Testing"
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Paused is not terminal and must keep the lease"
    );
}

#[test]
fn finalize_validation_command_batch_recovery_required_keeps_the_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::ExitFailure,
        Some(101),
        "validation command exited with a nonzero status",
    );

    let previous_state = task.state();
    task.transition_to(TaskState::RecoveryRequired, 200)
        .expect("Testing -> RecoveryRequired");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 200);

    repository
        .finalize_validation_command_batch(expected_version, &task, &record, &attempt)
        .expect("atomic recovery-required testing batch finalize");

    assert_eq!(
        repository
            .get_task(task.id())
            .expect("get task")
            .expect("task exists")
            .state(),
        TaskState::RecoveryRequired
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn finalize_validation_command_batch_rolls_back_everything_when_history_insert_fails() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, initial) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Success,
        Some(0),
        "cargo test passed",
    );

    let previous_state = task.state();
    task.transition_to(TaskState::Reviewing, 200)
        .expect("Testing -> Reviewing");
    // Reusing an already-persisted transition id forces the final history
    // insert to fail on its primary key after the result row has already
    // been written inside the same transaction.
    let duplicate_id_record = transition(initial.id(), &task, previous_state, 200);

    assert_code(
        repository
            .finalize_validation_command_batch(
                expected_version,
                &task,
                &duplicate_id_record,
                &attempt,
            )
            .expect_err("duplicate transition id must fail"),
        RepositoryErrorCode::OperationFailed,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task after rollback"),
        Some(before)
    );
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_validation_command_results"
        ),
        0
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease after rollback")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "a rolled-back write must not have released the lease"
    );
}

#[test]
fn finalize_validation_command_batch_rejects_version_mismatch_without_writing_a_result_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    let stale_expected_version = expected_version + 41;
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed an approval");
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Success,
        Some(0),
        "cargo test passed",
    );

    let previous_state = task.state();
    task.transition_to(TaskState::Reviewing, 200)
        .expect("Testing -> Reviewing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 200);

    assert_code(
        repository
            .finalize_validation_command_batch(stale_expected_version, &task, &record, &attempt)
            .expect_err("stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_validation_command_results"
        ),
        0
    );
}

#[test]
fn finalize_validation_command_batch_rejects_when_no_matching_approval_exists() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Success,
        Some(0),
        "cargo test passed",
    );

    let previous_state = task.state();
    task.transition_to(TaskState::Reviewing, 200)
        .expect("Testing -> Reviewing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 200);

    assert_code(
        repository
            .finalize_validation_command_batch(expected_version, &task, &record, &attempt)
            .expect_err("no Test approval was ever recorded for this task/version"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(task.id()).expect("task unchanged"),
        Some(before)
    );
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_validation_command_results"
        ),
        0
    );
}

#[test]
fn finalize_validation_command_batch_computes_the_next_attempt_sequence_after_an_existing_result() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Format,
            "cargo",
            &["fmt", "--all", "--check"],
        ))
        .expect("seed a Format approval");
    repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            expected_version,
            ValidationCommandKind::Format,
            ValidationCommandResultOutcome::Success,
            Some(0),
            "cargo fmt passed",
        ))
        .expect("seed an intermediate Format result at sequence 1");
    repository
        .save_validation_command_approval(&validation_command_approval(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            "cargo",
            &["test", "--workspace"],
        ))
        .expect("seed a Test approval");
    // Simulate a re-entrant batch: one earlier Test attempt already exists
    // (e.g. from a prior AutoFixing round), so the final attempt recorded
    // now must continue that same per-(task, version, kind) sequence.
    repository
        .append_validation_command_result(&validation_command_result_attempt(
            task.id(),
            expected_version,
            ValidationCommandKind::Test,
            ValidationCommandResultOutcome::ExitFailure,
            Some(1),
            "cargo test failed",
        ))
        .expect("seed a prior Test attempt at sequence 1");
    let attempt = validation_command_result_attempt(
        task.id(),
        expected_version,
        ValidationCommandKind::Test,
        ValidationCommandResultOutcome::Success,
        Some(0),
        "cargo test passed",
    );

    let previous_state = task.state();
    task.transition_to(TaskState::Reviewing, 200)
        .expect("Testing -> Reviewing");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 200);

    repository
        .finalize_validation_command_batch(expected_version, &task, &record, &attempt)
        .expect("atomic testing batch finalize");

    let stored = repository
        .list_validation_command_results(task.id(), expected_version, ValidationCommandKind::Test)
        .expect("read back results");
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].attempt_sequence, 1);
    assert_eq!(
        stored[0].outcome,
        ValidationCommandResultOutcome::ExitFailure
    );
    assert_eq!(stored[1].attempt_sequence, 2);
    assert_eq!(stored[1].outcome, ValidationCommandResultOutcome::Success);
}

#[test]
fn save_context_package_manifest_persists_and_is_readable_back_under_the_exact_five_tuple() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();

    let consent = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::ContextPackageV1,
            200,
        )
        .expect("record the ContextPackageV1 consent the manifest will reference");

    let manifest = ContextPackageManifestRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        approved_task_version: consent.approved_task_version,
        data_scope: ContextDataScope::ContextPackageV1,
        created_at_ms: 210,
    };
    repository
        .save_context_package_manifest(&manifest)
        .expect("save a manifest referencing an existing ContextPackageV1 consent");

    let stored = repository
        .get_context_package_manifest(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
            ContextDataScope::ContextPackageV1,
        )
        .expect("read the manifest back")
        .expect("the manifest just saved must be persisted and readable back");
    assert_eq!(stored, manifest);

    assert_eq!(
        repository
            .get_context_package_manifest(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read under the wrong scope"),
        None,
        "a ContextPackageV1 manifest must not satisfy a LegacyPhase4 lookup"
    );
    assert_eq!(
        repository
            .get_context_package_manifest(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .expect("read under the wrong work_kind"),
        None,
        "a Review manifest must not satisfy a Planning lookup for the same task/version/scope"
    );
    assert_eq!(
        repository
            .get_context_package_manifest(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version + 1,
                ContextDataScope::ContextPackageV1,
            )
            .expect("read under the wrong task version"),
        None,
        "a manifest for one task version must not satisfy a lookup for a different version"
    );
}

#[test]
fn save_context_package_manifest_rejects_when_matching_consent_is_missing() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();

    let manifest = ContextPackageManifestRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        approved_task_version: expected_version,
        data_scope: ContextDataScope::ContextPackageV1,
        created_at_ms: 210,
    };

    assert_code(
        repository
            .save_context_package_manifest(&manifest)
            .expect_err("no ContextPackageV1 consent exists yet for this identity"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository
            .get_context_package_manifest(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .expect("read manifest"),
        None,
        "a rejected save must never leave a partial manifest row behind"
    );
}

#[test]
fn save_context_package_manifest_rejects_legacy_phase4_scope_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let expected_version = task.version();

    repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            200,
        )
        .expect("record a LegacyPhase4 consent (a manifest must never be created for it)");

    let manifest = ContextPackageManifestRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        approved_task_version: expected_version,
        data_scope: ContextDataScope::LegacyPhase4,
        created_at_ms: 210,
    };

    assert_code(
        repository
            .save_context_package_manifest(&manifest)
            .expect_err("LegacyPhase4 must be rejected before ever reaching the database driver"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository
            .get_context_package_manifest(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read manifest"),
        None,
        "a rejected LegacyPhase4-scoped save must never leave a manifest row behind"
    );
}

#[test]
fn save_high_risk_approval_persists_and_is_readable_back_for_every_category_independently() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    for category in HighRiskCategory::ALL {
        let approval = HighRiskApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            risk_category: category,
            approved_at_ms: 210,
        };
        repository
            .save_high_risk_approval(&approval)
            .unwrap_or_else(|error| panic!("save {category:?} approval: {error:?}"));

        let stored = repository
            .get_high_risk_approval(task.id(), expected_version, category)
            .expect("read the approval back")
            .unwrap_or_else(|| panic!("{category:?} approval must be persisted and readable"));
        assert_eq!(stored, approval);
    }

    assert_eq!(
        repository
            .get_high_risk_approval(task.id(), expected_version, HighRiskCategory::DataMigration,)
            .expect("read under a different task id-scoped query"),
        Some(HighRiskApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            risk_category: HighRiskCategory::DataMigration,
            approved_at_ms: 210,
        }),
        "each category must remain independently readable after all 13 are stored"
    );
    assert_eq!(
        repository
            .get_high_risk_approval(
                TaskId::new(),
                expected_version,
                HighRiskCategory::DataMigration
            )
            .expect("read under an unrelated task id"),
        None,
        "an approval must never satisfy a lookup for a different task id"
    );
    assert_eq!(
        repository
            .get_high_risk_approval(
                task.id(),
                expected_version + 1,
                HighRiskCategory::DataMigration,
            )
            .expect("read under the wrong task version"),
        None,
        "an approval for one task version must not satisfy a lookup for a different version"
    );
}

#[test]
fn save_high_risk_approval_rejects_duplicate_identity_without_disturbing_the_existing_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let approval = HighRiskApprovalRecord {
        task_id: task.id(),
        approved_task_version: expected_version,
        risk_category: HighRiskCategory::ArchitectureChange,
        approved_at_ms: 210,
    };
    repository
        .save_high_risk_approval(&approval)
        .expect("save the first approval for this identity");

    let duplicate = HighRiskApprovalRecord {
        approved_at_ms: 999,
        ..approval
    };
    assert_code(
        repository
            .save_high_risk_approval(&duplicate)
            .expect_err("a second approval for the exact same identity must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository
            .get_high_risk_approval(
                task.id(),
                expected_version,
                HighRiskCategory::ArchitectureChange,
            )
            .expect("read the approval back")
            .expect("the original approval must remain"),
        approval,
        "a rejected duplicate save must never overwrite the original row"
    );
}

#[test]
fn save_high_risk_approval_rejects_a_stale_task_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    let stale_version = task.version();
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);

    let approval = HighRiskApprovalRecord {
        task_id: task.id(),
        approved_task_version: stale_version,
        risk_category: HighRiskCategory::DataMigration,
        approved_at_ms: 210,
    };
    assert_code(
        repository
            .save_high_risk_approval(&approval)
            .expect_err("an approval bound to a version other than the current one is rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository
            .get_high_risk_approval(task.id(), stale_version, HighRiskCategory::DataMigration)
            .expect("read the rejected approval"),
        None,
        "a rejected version-mismatched save must never leave a row behind"
    );
}

#[test]
fn save_high_risk_approval_never_changes_task_state_version_history_or_the_active_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let before_task = repository
        .get_task(task.id())
        .expect("read task before")
        .expect("task exists");
    let before_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history before");
    let before_lease = repository.active_lease().expect("read lease before");

    repository
        .save_high_risk_approval(&HighRiskApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            risk_category: HighRiskCategory::SecurityPolicyChange,
            approved_at_ms: 210,
        })
        .expect("save a high-risk approval");

    let after_task = repository
        .get_task(task.id())
        .expect("read task after")
        .expect("task still exists");
    let after_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history after");
    let after_lease = repository.active_lease().expect("read lease after");

    assert_eq!(before_task.state(), after_task.state());
    assert_eq!(before_task.version(), after_task.version());
    assert_eq!(before_transitions.len(), after_transitions.len());
    assert_eq!(
        before_lease.map(|lease| lease.task_id),
        after_lease.map(|lease| lease.task_id)
    );
}

#[test]
fn get_high_risk_approval_fails_closed_on_a_row_with_a_corrupted_risk_category() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();
    drop(connection);

    let raw = fixture.database.open_raw();
    raw.execute_batch("PRAGMA ignore_check_constraints = 1;")
        .expect("disable CHECK enforcement for this fixture connection only");
    raw.execute(
        "INSERT INTO task_high_risk_approvals (
            task_id, approved_task_version, risk_category, approved_at_ms
         ) VALUES (?1, ?2, 'NotACategory', 100)",
        params![task.id().to_string(), expected_version as i64],
    )
    .expect("insert a row the CHECK constraint would normally reject");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .get_high_risk_approval(task.id(), expected_version, HighRiskCategory::DataMigration)
        .expect_err("a corrupted risk_category must fail the lookup closed");
    assert_code(error, RepositoryErrorCode::InvalidPersistenceState);
}

#[test]
fn ensure_high_risk_approval_first_call_creates_the_approval() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let created = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::BreakingCompatibilityChange,
            210,
        )
        .expect("first call must create the approval");
    assert_eq!(
        created,
        HighRiskApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            risk_category: HighRiskCategory::BreakingCompatibilityChange,
            approved_at_ms: 210,
        }
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_high_risk_approvals"),
        1
    );
}

#[test]
fn ensure_high_risk_approval_reuses_the_exact_same_identity_idempotently_without_inserting_a_duplicate()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let first = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::DataMigration,
            210,
        )
        .expect("first call creates the approval");
    let second = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::DataMigration,
            999,
        )
        .expect("second call must reuse the existing approval, not fail or overwrite it");

    assert_eq!(
        first, second,
        "reusing an existing exact-identity approval must return it unchanged, ignoring the \
         second call's timestamp"
    );
    assert_eq!(
        second.approved_at_ms, 210,
        "the original timestamp must be preserved"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_high_risk_approvals"),
        1
    );
}

#[test]
fn ensure_high_risk_approval_for_different_categories_at_the_same_version_are_independent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let architecture = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::ArchitectureChange,
            210,
        )
        .expect("create the ArchitectureChange approval");
    let migration = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::DataMigration,
            220,
        )
        .expect("create the independent DataMigration approval");

    assert_ne!(architecture, migration);
    assert_eq!(
        repository
            .get_high_risk_approval(
                task.id(),
                expected_version,
                HighRiskCategory::ArchitectureChange
            )
            .expect("read ArchitectureChange back"),
        Some(architecture)
    );
    assert_eq!(
        repository
            .get_high_risk_approval(task.id(), expected_version, HighRiskCategory::DataMigration)
            .expect("read DataMigration back"),
        Some(migration)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_high_risk_approvals"),
        2
    );
}

#[test]
fn ensure_high_risk_approval_rejects_a_stale_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    let stale_version = task.version();
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);

    assert_code(
        repository
            .ensure_high_risk_approval(
                task.id(),
                stale_version,
                HighRiskCategory::DataMigration,
                210,
            )
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_high_risk_approvals"),
        0
    );
}

#[test]
fn ensure_high_risk_approval_never_changes_task_state_version_history_or_the_active_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let before_task = repository
        .get_task(task.id())
        .expect("read task before")
        .expect("task exists");
    let before_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history before");
    let before_lease = repository.active_lease().expect("read lease before");

    repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::SecurityPolicyChange,
            210,
        )
        .expect("create the approval");
    repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::SecurityPolicyChange,
            999,
        )
        .expect("reuse the approval");

    let after_task = repository
        .get_task(task.id())
        .expect("read task after")
        .expect("task still exists");
    let after_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history after");
    let after_lease = repository.active_lease().expect("read lease after");

    assert_eq!(before_task.state(), after_task.state());
    assert_eq!(before_task.version(), after_task.version());
    assert_eq!(before_transitions.len(), after_transitions.len());
    assert_eq!(
        before_lease.map(|lease| lease.task_id),
        after_lease.map(|lease| lease.task_id)
    );
}

#[test]
fn ensure_high_risk_approval_fails_closed_when_an_existing_row_for_the_task_version_is_corrupted() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();
    drop(connection);

    let raw = fixture.database.open_raw();
    raw.execute_batch("PRAGMA ignore_check_constraints = 1;")
        .expect("disable CHECK enforcement for this fixture connection only");
    raw.execute(
        "INSERT INTO task_high_risk_approvals (
            task_id, approved_task_version, risk_category, approved_at_ms
         ) VALUES (?1, ?2, 'NotACategory', 100)",
        params![task.id().to_string(), expected_version as i64],
    )
    .expect("insert a row the CHECK constraint would normally reject");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::DataMigration,
            210,
        )
        .expect_err(
            "a corrupted existing row for this task/version must fail the whole operation \
             closed rather than inserting a second, independent approval",
        );
    assert_code(error, RepositoryErrorCode::InvalidPersistenceState);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_high_risk_approvals"),
        1,
        "a fail-closed rejection must never leave a newly-inserted row behind"
    );
}

#[test]
fn two_connections_ensuring_the_same_high_risk_approval_concurrently_both_succeed_with_exactly_one_row()
 {
    let fixture = Fixture::new();
    let (task, expected_version) = {
        let mut connection = fixture.open();
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        let (task, _) = create_task(&mut repository, fixture.project_id);
        let expected_version = task.version();
        (task, expected_version)
    };

    let path = Arc::new(fixture.database.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for index in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        let task_id = task.id();
        handles.push(thread::spawn(move || {
            let mut database = DatabaseConnection::open(path.as_ref()).expect("open connection");
            let mut repository = SqliteFoundationRepository::new(&mut database);
            barrier.wait();
            let result = repository
                .ensure_high_risk_approval(
                    task_id,
                    expected_version,
                    HighRiskCategory::DataMigration,
                    // Distinct timestamps prove whichever thread loses the race reuses the
                    // winner's persisted value instead of its own.
                    200 + index as i64,
                )
                .map_err(|error| error.code());
            sender.send(result).expect("send ensure result");
        }));
    }
    drop(sender);

    let results = (0..2)
        .map(|_| {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrent ensure timed out")
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("ensure thread panicked");
    }

    let successes = results
        .iter()
        .filter(|result| result.is_ok())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        successes.len(),
        2,
        "an idempotent create-or-reuse must let both concurrent callers succeed: {results:?}"
    );
    assert_eq!(
        successes[0], successes[1],
        "both concurrent callers must observe the exact same persisted approval, proving no \
         duplicate PK failure was exposed as a distinct successful result"
    );

    let connection = fixture.database.open_raw();
    assert_eq!(count_rows(&connection, "task_high_risk_approvals"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

fn hash_of(byte: u8) -> DiffContentHash {
    DiffContentHash::from_digest_bytes([byte; 32])
}

#[test]
fn save_diff_approval_persists_and_is_readable_back_and_distinct_hashes_are_independent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let approval = DiffApprovalRecord {
        task_id: task.id(),
        approved_task_version: expected_version,
        diff_content_hash: hash_of(1),
        approved_at_ms: 210,
    };
    repository
        .save_diff_approval(&approval)
        .expect("save the first diff approval");

    let stored = repository
        .get_diff_approval(task.id(), expected_version, hash_of(1))
        .expect("read the approval back")
        .expect("the approval must be persisted and readable");
    assert_eq!(stored, approval);

    let other = DiffApprovalRecord {
        diff_content_hash: hash_of(2),
        approved_at_ms: 220,
        ..approval
    };
    repository
        .save_diff_approval(&other)
        .expect("a distinct hash at the same task/version is its own independent row");
    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash_of(1))
            .expect("read the first hash back"),
        Some(approval),
        "each hash must remain independently readable after a second, distinct hash is stored"
    );
    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash_of(2))
            .expect("read the second hash back"),
        Some(other)
    );
    assert_eq!(
        repository
            .get_diff_approval(TaskId::new(), expected_version, hash_of(1))
            .expect("read under an unrelated task id"),
        None,
        "an approval must never satisfy a lookup for a different task id"
    );
    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version + 1, hash_of(1))
            .expect("read under the wrong task version"),
        None,
        "an approval for one task version must not satisfy a lookup for a different version"
    );
    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash_of(3))
            .expect("read under an unrelated hash"),
        None,
        "an approval must never satisfy a lookup for a different hash"
    );
}

#[test]
fn save_diff_approval_rejects_duplicate_identity_without_disturbing_the_existing_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let approval = DiffApprovalRecord {
        task_id: task.id(),
        approved_task_version: expected_version,
        diff_content_hash: hash_of(1),
        approved_at_ms: 210,
    };
    repository
        .save_diff_approval(&approval)
        .expect("save the first approval for this identity");

    let duplicate = DiffApprovalRecord {
        approved_at_ms: 999,
        ..approval
    };
    assert_code(
        repository
            .save_diff_approval(&duplicate)
            .expect_err("a second approval for the exact same identity must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash_of(1))
            .expect("read the approval back")
            .expect("the original approval must remain"),
        approval,
        "a rejected duplicate save must never overwrite the original row"
    );
}

#[test]
fn save_diff_approval_rejects_a_stale_task_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    let stale_version = task.version();
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);

    let approval = DiffApprovalRecord {
        task_id: task.id(),
        approved_task_version: stale_version,
        diff_content_hash: hash_of(1),
        approved_at_ms: 210,
    };
    assert_code(
        repository
            .save_diff_approval(&approval)
            .expect_err("an approval bound to a version other than the current one is rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository
            .get_diff_approval(task.id(), stale_version, hash_of(1))
            .expect("read the rejected approval"),
        None,
        "a rejected version-mismatched save must never leave a row behind"
    );
}

#[test]
fn save_diff_approval_never_changes_task_state_version_history_or_the_active_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let before_task = repository
        .get_task(task.id())
        .expect("read task before")
        .expect("task exists");
    let before_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history before");
    let before_lease = repository.active_lease().expect("read lease before");

    repository
        .save_diff_approval(&DiffApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            diff_content_hash: hash_of(1),
            approved_at_ms: 210,
        })
        .expect("save a diff approval");

    let after_task = repository
        .get_task(task.id())
        .expect("read task after")
        .expect("task still exists");
    let after_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history after");
    let after_lease = repository.active_lease().expect("read lease after");

    assert_eq!(before_task.state(), after_task.state());
    assert_eq!(before_task.version(), after_task.version());
    assert_eq!(before_transitions.len(), after_transitions.len());
    assert_eq!(
        before_lease.map(|lease| lease.task_id),
        after_lease.map(|lease| lease.task_id)
    );
}

#[test]
fn get_diff_approval_fails_closed_on_a_row_with_a_malformed_persisted_hash() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();
    drop(connection);

    let raw = fixture.database.open_raw();
    raw.execute_batch("PRAGMA ignore_check_constraints = 1;")
        .expect("disable CHECK enforcement for this fixture connection only");
    raw.execute(
        "INSERT INTO task_diff_approvals (
            task_id, approved_task_version, diff_content_hash_hex, approved_at_ms
         ) VALUES (?1, ?2, 'not-a-valid-hex-hash', 100)",
        params![task.id().to_string(), expected_version as i64],
    )
    .expect("insert a row the CHECK constraint would normally reject");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .get_diff_approval(task.id(), expected_version, hash_of(1))
        .expect_err("a malformed persisted hash must fail the lookup closed");
    assert_code(error, RepositoryErrorCode::InvalidPersistenceState);
}

#[test]
fn ensure_diff_approval_first_call_creates_the_approval() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let created = repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 210)
        .expect("first call must create the approval");
    assert_eq!(
        created,
        DiffApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            diff_content_hash: hash_of(1),
            approved_at_ms: 210,
        }
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_diff_approvals"),
        1
    );
}

#[test]
fn ensure_diff_approval_reuses_the_exact_same_identity_idempotently_without_inserting_a_duplicate()
{
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let first = repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 210)
        .expect("first call creates the approval");
    let second = repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 999)
        .expect("second call must reuse the existing approval, not fail or overwrite it");

    assert_eq!(
        first, second,
        "reusing an existing exact-identity approval must return it unchanged, ignoring the \
         second call's timestamp"
    );
    assert_eq!(
        second.approved_at_ms, 210,
        "the original timestamp must be preserved"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_diff_approvals"),
        1
    );
}

#[test]
fn ensure_diff_approval_for_different_hashes_at_the_same_version_are_independent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let first = repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 210)
        .expect("create the first hash's approval");
    let second = repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(2), 220)
        .expect("create the independent second hash's approval");

    assert_ne!(first, second);
    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash_of(1))
            .expect("read the first hash back"),
        Some(first)
    );
    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash_of(2))
            .expect("read the second hash back"),
        Some(second)
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_diff_approvals"),
        2
    );
}

#[test]
fn ensure_diff_approval_rejects_a_stale_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    let stale_version = task.version();
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);

    assert_code(
        repository
            .ensure_diff_approval(task.id(), stale_version, hash_of(1), 210)
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_diff_approvals"),
        0
    );
}

#[test]
fn ensure_diff_approval_never_changes_task_state_version_history_or_the_active_lease() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let before_task = repository
        .get_task(task.id())
        .expect("read task before")
        .expect("task exists");
    let before_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history before");
    let before_lease = repository.active_lease().expect("read lease before");

    repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 210)
        .expect("create the approval");
    repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 999)
        .expect("reuse the approval");

    let after_task = repository
        .get_task(task.id())
        .expect("read task after")
        .expect("task still exists");
    let after_transitions = repository
        .list_task_transitions(task.id())
        .expect("read history after");
    let after_lease = repository.active_lease().expect("read lease after");

    assert_eq!(before_task.state(), after_task.state());
    assert_eq!(before_task.version(), after_task.version());
    assert_eq!(before_transitions.len(), after_transitions.len());
    assert_eq!(
        before_lease.map(|lease| lease.task_id),
        after_lease.map(|lease| lease.task_id)
    );
}

#[test]
fn ensure_diff_approval_fails_closed_when_an_existing_row_for_the_task_version_is_corrupted() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();
    drop(connection);

    let raw = fixture.database.open_raw();
    raw.execute_batch("PRAGMA ignore_check_constraints = 1;")
        .expect("disable CHECK enforcement for this fixture connection only");
    raw.execute(
        "INSERT INTO task_diff_approvals (
            task_id, approved_task_version, diff_content_hash_hex, approved_at_ms
         ) VALUES (?1, ?2, 'not-a-valid-hex-hash', 100)",
        params![task.id().to_string(), expected_version as i64],
    )
    .expect("insert a row the CHECK constraint would normally reject");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .ensure_diff_approval(task.id(), expected_version, hash_of(1), 210)
        .expect_err(
            "a corrupted existing row for this task/version must fail the whole operation \
             closed rather than treating it as absent and inserting a second, independent \
             approval",
        );
    assert_code(error, RepositoryErrorCode::InvalidPersistenceState);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_diff_approvals"),
        1,
        "a fail-closed rejection must never leave a newly-inserted row behind"
    );
}

#[test]
fn two_connections_ensuring_the_same_diff_approval_concurrently_both_succeed_with_exactly_one_row()
{
    let fixture = Fixture::new();
    let (task, expected_version) = {
        let mut connection = fixture.open();
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        let (task, _) = create_task(&mut repository, fixture.project_id);
        let expected_version = task.version();
        (task, expected_version)
    };

    let path = Arc::new(fixture.database.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for index in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        let task_id = task.id();
        handles.push(thread::spawn(move || {
            let mut database = DatabaseConnection::open(path.as_ref()).expect("open connection");
            let mut repository = SqliteFoundationRepository::new(&mut database);
            barrier.wait();
            let result = repository
                .ensure_diff_approval(
                    task_id,
                    expected_version,
                    hash_of(1),
                    // Distinct timestamps prove whichever thread loses the race reuses the
                    // winner's persisted value instead of its own.
                    200 + index as i64,
                )
                .map_err(|error| error.code());
            sender.send(result).expect("send ensure result");
        }));
    }
    drop(sender);

    let results = (0..2)
        .map(|_| {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrent ensure timed out")
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("ensure thread panicked");
    }

    let successes = results
        .iter()
        .filter(|result| result.is_ok())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        successes.len(),
        2,
        "an idempotent create-or-reuse must let both concurrent callers succeed: {results:?}"
    );
    assert_eq!(
        successes[0], successes[1],
        "both concurrent callers must observe the exact same persisted approval, proving no \
         duplicate PK failure was exposed as a distinct successful result"
    );

    let connection = fixture.database.open_raw();
    assert_eq!(count_rows(&connection, "task_diff_approvals"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

/// Advances a freshly created task through the real Git isolation state
/// machine up to `WorktreeCreating` (task state *and* a real
/// `task_git_isolations` row at the matching status), returning both. Unlike
/// this file's `advance_to_worktree_ready` (a plain `save_transition` bump
/// used everywhere else, which never creates an isolation row at all),
/// `prepare_planning_context_package` re-verifies a real isolation record,
/// so tests of it need one to actually exist.
fn worktree_creating_task_with_real_isolation(
    repository: &mut impl FoundationRepository,
    project_id: ProjectId,
) -> (Task, TaskGitIsolation) {
    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    task.transition_to(TaskState::ProjectValidated, 101)
        .expect("classify project");
    let classified = transition(TaskStateTransitionId::new(), &task, TaskState::Created, 101);
    let mut isolation = TaskGitIsolation {
        task_id: task.id(),
        project_id,
        status: GitIsolationStatus::Ready,
        operation_id: None,
        expected_task_version: 1,
        base_branch: None,
        base_commit: None,
        worktree_path: None,
        branch_created_by_app: false,
        worktree_created_by_app: false,
        created_at_ms: 100,
        updated_at_ms: 101,
    };
    repository
        .create_isolation_task(&task, &initial, &classified, 100, &isolation, None)
        .expect("create isolation task");

    let previous = task.state();
    task.transition_to(TaskState::WorktreeCreating, 102)
        .expect("worktree intent state");
    let worktree_transition = transition(TaskStateTransitionId::new(), &task, previous, 102);
    isolation.status = GitIsolationStatus::WorktreeCreating;
    isolation.operation_id = Some(chatoms_domain::GitOperationId::new());
    isolation.expected_task_version = task.version();
    isolation.base_branch = Some("main".to_owned());
    isolation.base_commit = Some("a".repeat(40));
    isolation.worktree_path = Some("C:/managed/project/task".to_owned());
    isolation.updated_at_ms = 102;
    repository
        .save_isolation_transition(1, &task, &worktree_transition, &isolation)
        .expect("worktree creating transition");

    (task, isolation)
}

/// Continues [`worktree_creating_task_with_real_isolation`] through to
/// `WorktreeReady` (task state *and* isolation status), returning the final
/// task.
fn worktree_ready_task_with_real_isolation(
    repository: &mut impl FoundationRepository,
    project_id: ProjectId,
) -> Task {
    let (mut task, mut isolation) =
        worktree_creating_task_with_real_isolation(repository, project_id);
    let previous = task.state();
    task.transition_to(TaskState::WorktreeReady, 103)
        .expect("worktree ready state");
    let ready_transition = transition(TaskStateTransitionId::new(), &task, previous, 103);
    isolation.status = GitIsolationStatus::WorktreeReady;
    isolation.expected_task_version = task.version();
    isolation.branch_created_by_app = true;
    isolation.worktree_created_by_app = true;
    isolation.updated_at_ms = 103;
    repository
        .save_worktree_completion(2, &task, &ready_transition, &isolation)
        .expect("worktree ready completion");

    task
}

#[test]
fn prepare_planning_context_package_creates_consent_and_manifest_atomically_when_neither_exists() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let preparation = repository
        .prepare_planning_context_package(task.version(), task.id(), 200)
        .expect("atomic preparation when neither consent nor manifest exists");

    assert_eq!(preparation.consent.task_id, task.id());
    assert_eq!(preparation.consent.provider, ProviderKind::Claude);
    assert_eq!(preparation.consent.work_kind, WorkKind::Planning);
    assert_eq!(preparation.consent.approved_task_version, task.version());
    assert_eq!(
        preparation.consent.data_scope,
        ContextDataScope::ContextPackageV1
    );
    assert_eq!(preparation.consent.consented_at_ms, 200);
    assert_eq!(preparation.manifest.task_id, task.id());
    assert_eq!(preparation.manifest.provider, ProviderKind::Claude);
    assert_eq!(preparation.manifest.work_kind, WorkKind::Planning);
    assert_eq!(preparation.manifest.approved_task_version, task.version());
    assert_eq!(
        preparation.manifest.data_scope,
        ContextDataScope::ContextPackageV1
    );
    assert_eq!(preparation.manifest.created_at_ms, 200);

    assert_eq!(
        repository.get_task(task.id()).expect("task"),
        Some(before),
        "preparation must never change task state or version"
    );
    assert_eq!(
        repository
            .list_task_transitions(task.id())
            .expect("history after"),
        history_before,
        "preparation must never add a transition history entry"
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("lease")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "preparation must never touch the ActiveTaskLease"
    );
}

#[test]
fn prepare_planning_context_package_reuses_existing_pair_idempotently_without_mutation() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);

    let first = repository
        .prepare_planning_context_package(task.version(), task.id(), 200)
        .expect("first preparation");
    let second = repository
        .prepare_planning_context_package(task.version(), task.id(), 999)
        .expect("second preparation must reuse the existing pair, not fail");

    assert_eq!(
        second, first,
        "reusing an existing pair must return it unchanged, not the new prepared_at_ms"
    );
    assert_eq!(second.consent.consented_at_ms, 200);
    assert_eq!(second.manifest.created_at_ms, 200);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        1
    );
}

#[test]
fn prepare_planning_context_package_rejects_consent_only_partial_state_without_repairing_manifest()
{
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let raw = fixture.database.open_raw();
    raw.execute(
        "INSERT INTO task_provider_consents (
            task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
         ) VALUES (?1, 'Claude', 'Planning', ?2, 'ContextPackageV1', 150)",
        params![
            task.id().to_string(),
            i64::try_from(task.version()).unwrap()
        ],
    )
    .expect("seed a consent-only partial state");
    drop(raw);

    assert_code(
        repository
            .prepare_planning_context_package(task.version(), task.id(), 200)
            .expect_err("a consent-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1,
        "the lone existing consent must not be duplicated"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        0,
        "the missing manifest must never be silently inserted to repair the pair"
    );
}

#[test]
fn prepare_planning_context_package_rejects_manifest_only_partial_state_without_repairing_consent()
{
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);

    // The manifest's foreign key into task_provider_consents makes this
    // state unreachable through any real, FK-enforced write path -- that
    // FK is the whole point of Unit 5a-2 -- so this is purely a
    // defense-in-depth reproduction: foreign_keys is deliberately disabled
    // on a throwaway connection just long enough to force the row into
    // existence, then dropped before the real (FK-enforced) repository
    // connection is used again.
    let bypass =
        rusqlite::Connection::open(fixture.database.path()).expect("open bypass connection");
    bypass
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("disable FK enforcement for this throwaway connection only");
    bypass
        .execute(
            "INSERT INTO context_package_manifests (
                task_id, provider, work_kind, approved_task_version, data_scope, created_at_ms
             ) VALUES (?1, 'Claude', 'Planning', ?2, 'ContextPackageV1', 150)",
            params![
                task.id().to_string(),
                i64::try_from(task.version()).unwrap()
            ],
        )
        .expect("force a manifest-only partial state despite the foreign key");
    drop(bypass);

    assert_code(
        repository
            .prepare_planning_context_package(task.version(), task.id(), 200)
            .expect_err("a manifest-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0,
        "the missing consent must never be silently inserted to repair the pair"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        1,
        "the lone existing manifest must not be duplicated"
    );
}

#[test]
fn prepare_planning_context_package_rejects_wrong_task_state_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_planning(&mut repository, &mut task);

    assert_code(
        repository
            .prepare_planning_context_package(task.version(), task.id(), 200)
            .expect_err("Planning must not be accepted as WorktreeReady"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        0
    );
}

#[test]
fn prepare_planning_context_package_rejects_stale_expected_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let stale_version = task.version() + 41;

    assert_code(
        repository
            .prepare_planning_context_package(stale_version, task.id(), 200)
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0
    );
}

#[test]
fn prepare_planning_context_package_rejects_a_non_worktree_ready_isolation_status() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _isolation) =
        worktree_creating_task_with_real_isolation(&mut repository, fixture.project_id);
    // Forces the task itself to WorktreeReady via a plain transition while
    // leaving the isolation record at WorktreeCreating -- a mismatch the
    // real isolation state machine cannot produce, but one that exercises
    // this method's own independent isolation re-check.
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 103);

    assert_code(
        repository
            .prepare_planning_context_package(task.version(), task.id(), 200)
            .expect_err("a non-WorktreeReady isolation status must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0
    );
}

#[test]
fn prepare_implementation_context_package_creates_and_then_reuses_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let before = task.clone();

    let first = repository
        .prepare_implementation_context_package(task.version(), task.id(), 200)
        .expect("first preparation");
    assert_eq!(first.consent.work_kind, WorkKind::Implementation);
    assert_eq!(first.consent.data_scope, ContextDataScope::ContextPackageV1);
    assert_eq!(first.manifest.work_kind, WorkKind::Implementation);

    let second = repository
        .prepare_implementation_context_package(task.version(), task.id(), 999)
        .expect("second preparation must reuse the existing pair");
    assert_eq!(second, first);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        1
    );
    assert_eq!(
        repository.get_task(task.id()).expect("task"),
        Some(before),
        "preparation must never change task state or version"
    );
}

#[test]
fn prepare_implementation_context_package_rejects_consent_only_partial_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let raw = fixture.database.open_raw();
    raw.execute(
        "INSERT INTO task_provider_consents (
            task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
         ) VALUES (?1, 'Claude', 'Implementation', ?2, 'ContextPackageV1', 150)",
        params![
            task.id().to_string(),
            i64::try_from(task.version()).unwrap()
        ],
    )
    .expect("seed a consent-only partial state");
    drop(raw);

    assert_code(
        repository
            .prepare_implementation_context_package(task.version(), task.id(), 200)
            .expect_err("a consent-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        0
    );
}

#[test]
fn prepare_implementation_context_package_rejects_wrong_task_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_worktree_ready(&mut repository, &mut task);

    assert_code(
        repository
            .prepare_implementation_context_package(task.version(), task.id(), 200)
            .expect_err("WorktreeReady must not be accepted as AwaitingDesignApproval"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0
    );
}

#[test]
fn prepare_implementation_context_package_rejects_stale_expected_version() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_design_approval(&mut repository, &mut task);
    let stale_version = task.version() + 41;

    assert_code(
        repository
            .prepare_implementation_context_package(stale_version, task.id(), 200)
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );
}

#[test]
fn prepare_review_context_package_creates_and_then_reuses_atomically() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let before = task.clone();

    let first = repository
        .prepare_review_context_package(task.version(), task.id(), 200)
        .expect("first preparation");
    assert_eq!(first.consent.work_kind, WorkKind::Review);
    assert_eq!(first.consent.data_scope, ContextDataScope::ContextPackageV1);
    assert_eq!(first.manifest.work_kind, WorkKind::Review);

    let second = repository
        .prepare_review_context_package(task.version(), task.id(), 999)
        .expect("second preparation must reuse the existing pair");
    assert_eq!(second, first);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        1
    );
    assert_eq!(
        repository.get_task(task.id()).expect("task"),
        Some(before),
        "preparation must never change task state or version"
    );
}

#[test]
fn prepare_review_context_package_rejects_consent_only_partial_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let raw = fixture.database.open_raw();
    raw.execute(
        "INSERT INTO task_provider_consents (
            task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
         ) VALUES (?1, 'Claude', 'Review', ?2, 'ContextPackageV1', 150)",
        params![
            task.id().to_string(),
            i64::try_from(task.version()).unwrap()
        ],
    )
    .expect("seed a consent-only partial state");
    drop(raw);

    assert_code(
        repository
            .prepare_review_context_package(task.version(), task.id(), 200)
            .expect_err("a consent-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        0
    );
}

#[test]
fn prepare_review_context_package_rejects_wrong_task_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_testing(&mut repository, &mut task);

    assert_code(
        repository
            .prepare_review_context_package(task.version(), task.id(), 200)
            .expect_err("Testing must not be accepted as Reviewing"),
        RepositoryErrorCode::InvalidAggregate,
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0
    );
}

#[test]
fn prepare_review_context_package_rejects_stale_expected_version() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);
    let stale_version = task.version() + 41;

    assert_code(
        repository
            .prepare_review_context_package(stale_version, task.id(), 200)
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );
}

#[test]
fn context_package_v1_preparation_never_reuses_or_touches_a_legacy_phase4_consent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_reviewing(&mut repository, &mut task);

    let legacy = repository
        .save_review_consent(
            task.version(),
            task.id(),
            ContextDataScope::LegacyPhase4,
            100,
        )
        .expect("seed a LegacyPhase4 consent");

    let preparation = repository
        .prepare_review_context_package(task.version(), task.id(), 200)
        .expect(
            "ContextPackageV1 preparation must succeed independently of the LegacyPhase4 consent",
        );

    assert_eq!(
        preparation.consent.data_scope,
        ContextDataScope::ContextPackageV1
    );
    assert_ne!(preparation.consent, legacy);

    let reloaded_legacy = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Review,
            task.version(),
            ContextDataScope::LegacyPhase4,
        )
        .expect("read legacy consent")
        .expect("legacy consent still present");
    assert_eq!(
        reloaded_legacy, legacy,
        "the LegacyPhase4 consent must be untouched by ContextPackageV1 preparation"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        2,
        "both scopes coexist as independent rows"
    );
}

/// Advances `task` from `WorktreeReady` (with a real `WorktreeReady`
/// isolation row) locally to `Planning`, returning the transition record a
/// caller of `save_context_package_planning_transition` would build. Mirrors
/// how `save_planning_transition`'s own tests build the `task`/`transition`
/// pair, but this Unit's tests all need the real isolation row
/// `worktree_ready_task_with_real_isolation` creates.
fn transition_to_planning(task: &mut Task, occurred_at_ms: i64) -> TaskStateTransition {
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, occurred_at_ms)
        .expect("WorktreeReady -> Planning");
    transition(
        TaskStateTransitionId::new(),
        task,
        previous_state,
        occurred_at_ms,
    )
}

#[test]
fn save_context_package_planning_transition_commits_when_the_exact_pair_exists_and_never_touches_a_legacy_consent()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let expected_version = task.version();
    repository
        .prepare_planning_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact ContextPackageV1 pair first");
    let history_before_len = repository
        .list_task_transitions(task.id())
        .expect("history before")
        .len();

    let record = transition_to_planning(&mut task, 210);

    repository
        .save_context_package_planning_transition(expected_version, &task, &record)
        .expect("commit when the exact pair already exists");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    let history = repository
        .list_task_transitions(task.id())
        .expect("history after");
    assert_eq!(history.len(), history_before_len + 1);
    assert_eq!(history.last().expect("latest transition"), &record);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1,
        "this method must never insert a LegacyPhase4 consent of its own"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Planning,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read legacy consent"),
        None,
        "no LegacyPhase4 consent must ever be created by this method"
    );
}

#[test]
fn save_context_package_planning_transition_rejects_when_preparation_is_entirely_missing() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let expected_version = task.version();
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_planning(&mut task, 210);

    assert_code(
        repository
            .save_context_package_planning_transition(expected_version, &task, &record)
            .expect_err("an entirely unprepared task must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone()),
        "the task must stay exactly WorktreeReady"
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before,
        "no transition history entry may be written"
    );
}

#[test]
fn save_context_package_planning_transition_rejects_a_consent_only_partial_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let expected_version = task.version();
    let before = task.clone();
    fixture
        .database
        .open_raw()
        .execute(
            "INSERT INTO task_provider_consents (
                task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
             ) VALUES (?1, 'Claude', 'Planning', ?2, 'ContextPackageV1', 150)",
            params![
                task.id().to_string(),
                i64::try_from(expected_version).unwrap()
            ],
        )
        .expect("seed a consent-only partial state");
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_planning(&mut task, 210);

    assert_code(
        repository
            .save_context_package_planning_transition(expected_version, &task, &record)
            .expect_err("a consent-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        0,
        "the missing manifest must never be silently inserted to repair the pair"
    );
}

#[test]
fn save_context_package_planning_transition_rejects_a_manifest_only_partial_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let expected_version = task.version();
    let before = task.clone();
    // Same defense-in-depth reproduction as
    // `prepare_planning_context_package_rejects_manifest_only_partial_state_without_repairing_consent`:
    // the manifest's foreign key makes this unreachable through any real,
    // FK-enforced write path, so foreign_keys is disabled on a throwaway
    // connection just long enough to force the row into existence.
    let bypass =
        rusqlite::Connection::open(fixture.database.path()).expect("open bypass connection");
    bypass
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("disable FK enforcement for this throwaway connection only");
    bypass
        .execute(
            "INSERT INTO context_package_manifests (
                task_id, provider, work_kind, approved_task_version, data_scope, created_at_ms
             ) VALUES (?1, 'Claude', 'Planning', ?2, 'ContextPackageV1', 150)",
            params![
                task.id().to_string(),
                i64::try_from(expected_version).unwrap()
            ],
        )
        .expect("force a manifest-only partial state despite the foreign key");
    drop(bypass);
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_planning(&mut task, 210);

    assert_code(
        repository
            .save_context_package_planning_transition(expected_version, &task, &record)
            .expect_err("a manifest-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0,
        "the missing consent must never be silently inserted to repair the pair"
    );
}

#[test]
fn save_context_package_planning_transition_rejects_a_stale_expected_version_without_writing_anything()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let actual_version = task.version();
    repository
        .prepare_planning_context_package(actual_version, task.id(), 200)
        .expect("prepare the exact pair at the real current version");
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");
    let stale_version = actual_version + 41;

    let record = transition_to_planning(&mut task, 210);

    assert_code(
        repository
            .save_context_package_planning_transition(stale_version, &task, &record)
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

#[test]
fn save_context_package_planning_transition_rejects_a_non_worktree_ready_isolation_status() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _isolation) =
        worktree_creating_task_with_real_isolation(&mut repository, fixture.project_id);
    // Forces the task itself to WorktreeReady via a plain transition while
    // leaving the isolation record at WorktreeCreating, the same mismatch
    // `prepare_planning_context_package_rejects_a_non_worktree_ready_isolation_status`
    // exercises.
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 103);
    let expected_version = task.version();
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_planning(&mut task, 210);

    assert_code(
        repository
            .save_context_package_planning_transition(expected_version, &task, &record)
            .expect_err("a non-WorktreeReady isolation status must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

#[test]
fn save_context_package_planning_transition_rejects_a_task_argument_not_already_reflecting_planning()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    let expected_version = task.version();
    repository
        .prepare_planning_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact pair");
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");
    // `task` here is still `WorktreeReady` -- the caller forgot to call
    // `transition_to(Planning, ...)` locally first. This method's own first
    // guard must reject rather than silently transitioning it anyway.
    let record = transition(
        TaskStateTransitionId::new(),
        &task,
        TaskState::WorktreeReady,
        210,
    );

    assert_code(
        repository
            .save_context_package_planning_transition(expected_version, &task, &record)
            .expect_err("a task argument that is not already Planning must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

/// Advances `task` (already `WorktreeReady` with a real isolation row, as
/// `worktree_ready_task_with_real_isolation` returns) through `Planning` to
/// `AwaitingDesignApproval`, atomically storing a `Completed`
/// `task_planning_results` row with non-empty `plan_text` via
/// `save_planning_result` — the same path the real Claude Planning flow uses
/// to reach this state, so `save_context_package_implementation_transition`'s
/// stored-plan precondition is satisfied the same way it would be in
/// production.
fn awaiting_design_approval_task_with_completed_plan(
    repository: &mut impl FoundationRepository,
    project_id: ProjectId,
    plan_text: &str,
) -> Task {
    let mut task = worktree_ready_task_with_real_isolation(repository, project_id);
    advance(repository, &mut task, TaskState::Planning, 140);
    let expected_version = task.version();
    let result = completed_planning_result(task.id(), plan_text);
    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 150);
    repository
        .save_planning_result(expected_version, &task, &record, &result, false)
        .expect("atomic planning result");
    task
}

/// Advances `task` from `AwaitingDesignApproval` locally to `Implementing`,
/// returning the transition record a caller of
/// `save_context_package_implementation_transition` would build. Mirrors
/// `transition_to_planning`.
fn transition_to_implementing(task: &mut Task, occurred_at_ms: i64) -> TaskStateTransition {
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, occurred_at_ms)
        .expect("AwaitingDesignApproval -> Implementing");
    transition(
        TaskStateTransitionId::new(),
        task,
        previous_state,
        occurred_at_ms,
    )
}

#[test]
fn save_context_package_implementation_transition_commits_when_the_exact_pair_and_completed_plan_exist_and_never_touches_a_legacy_consent()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = awaiting_design_approval_task_with_completed_plan(
        &mut repository,
        fixture.project_id,
        "a stored plan",
    );
    let expected_version = task.version();
    repository
        .prepare_implementation_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact ContextPackageV1 pair first");
    let history_before_len = repository
        .list_task_transitions(task.id())
        .expect("history before")
        .len();

    let record = transition_to_implementing(&mut task, 210);

    repository
        .save_context_package_implementation_transition(expected_version, &task, &record)
        .expect("commit when the exact pair and a completed plan already exist");

    assert_eq!(
        repository.get_task(task.id()).expect("get task"),
        Some(task.clone())
    );
    let history = repository
        .list_task_transitions(task.id())
        .expect("history after");
    assert_eq!(history.len(), history_before_len + 1);
    assert_eq!(history.last().expect("latest transition"), &record);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        1,
        "this method must never insert a LegacyPhase4 consent of its own"
    );
    assert_eq!(
        repository
            .get_provider_consent(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Implementation,
                expected_version,
                ContextDataScope::LegacyPhase4,
            )
            .expect("read legacy consent"),
        None,
        "no LegacyPhase4 consent must ever be created by this method"
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_when_preparation_is_entirely_missing() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = awaiting_design_approval_task_with_completed_plan(
        &mut repository,
        fixture.project_id,
        "a stored plan",
    );
    let expected_version = task.version();
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("an entirely unprepared task must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone()),
        "the task must stay exactly AwaitingDesignApproval"
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before,
        "no transition history entry may be written"
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_a_consent_only_partial_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = awaiting_design_approval_task_with_completed_plan(
        &mut repository,
        fixture.project_id,
        "a stored plan",
    );
    let expected_version = task.version();
    let before = task.clone();
    fixture
        .database
        .open_raw()
        .execute(
            "INSERT INTO task_provider_consents (
                task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
             ) VALUES (?1, 'Claude', 'Implementation', ?2, 'ContextPackageV1', 150)",
            params![
                task.id().to_string(),
                i64::try_from(expected_version).unwrap()
            ],
        )
        .expect("seed a consent-only partial state");
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("a consent-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "context_package_manifests"),
        0,
        "the missing manifest must never be silently inserted to repair the pair"
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_a_manifest_only_partial_state() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = awaiting_design_approval_task_with_completed_plan(
        &mut repository,
        fixture.project_id,
        "a stored plan",
    );
    let expected_version = task.version();
    let before = task.clone();
    // Same defense-in-depth reproduction as
    // `save_context_package_planning_transition_rejects_a_manifest_only_partial_state`:
    // the manifest's foreign key makes this unreachable through any real,
    // FK-enforced write path, so foreign_keys is disabled on a throwaway
    // connection just long enough to force the row into existence.
    let bypass =
        rusqlite::Connection::open(fixture.database.path()).expect("open bypass connection");
    bypass
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("disable FK enforcement for this throwaway connection only");
    bypass
        .execute(
            "INSERT INTO context_package_manifests (
                task_id, provider, work_kind, approved_task_version, data_scope, created_at_ms
             ) VALUES (?1, 'Claude', 'Implementation', ?2, 'ContextPackageV1', 150)",
            params![
                task.id().to_string(),
                i64::try_from(expected_version).unwrap()
            ],
        )
        .expect("force a manifest-only partial state despite the foreign key");
    drop(bypass);
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("a manifest-only partial state must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_provider_consents"),
        0,
        "the missing consent must never be silently inserted to repair the pair"
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_a_stale_expected_version_without_writing_anything()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = awaiting_design_approval_task_with_completed_plan(
        &mut repository,
        fixture.project_id,
        "a stored plan",
    );
    let actual_version = task.version();
    repository
        .prepare_implementation_context_package(actual_version, task.id(), 200)
        .expect("prepare the exact pair at the real current version");
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");
    let stale_version = actual_version + 41;

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(stale_version, &task, &record)
            .expect_err("a stale expected_version must be rejected"),
        RepositoryErrorCode::VersionConflict,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_a_non_worktree_ready_isolation_status() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _isolation) =
        worktree_creating_task_with_real_isolation(&mut repository, fixture.project_id);
    // Forces the task itself past WorktreeReady while leaving the isolation
    // record at WorktreeCreating, the same mismatch
    // `save_context_package_planning_transition_rejects_a_non_worktree_ready_isolation_status`
    // exercises.
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 103);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    let expected_version = task.version();
    let result = completed_planning_result(task.id(), "a stored plan");
    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 150);
    repository
        .save_planning_result(expected_version, &task, &record, &result, false)
        .expect("atomic planning result");
    repository
        .prepare_implementation_context_package(task.version(), task.id(), 200)
        .expect("prepare the exact pair");
    let expected_version = task.version();
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("a non-WorktreeReady isolation status must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_a_task_argument_not_already_reflecting_implementing()
 {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let task = awaiting_design_approval_task_with_completed_plan(
        &mut repository,
        fixture.project_id,
        "a stored plan",
    );
    let expected_version = task.version();
    repository
        .prepare_implementation_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact pair");
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");
    // `task` here is still `AwaitingDesignApproval` -- the caller forgot to
    // call `transition_to(Implementing, ...)` locally first. This method's
    // own first guard must reject rather than silently transitioning it
    // anyway.
    let record = transition(
        TaskStateTransitionId::new(),
        &task,
        TaskState::AwaitingDesignApproval,
        210,
    );

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("a task argument that is not already Implementing must be rejected"),
        RepositoryErrorCode::InvalidAggregate,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_when_no_planning_result_is_stored() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    // Reaches AwaitingDesignApproval via a plain domain transition, never
    // calling `save_planning_result` -- so no `task_planning_results` row
    // exists at all, a data-corruption scenario a real Claude Planning
    // completion could never itself produce (`AwaitingDesignApproval` is
    // only reachable through `save_planning_result`'s `Completed` path).
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    let expected_version = task.version();
    repository
        .prepare_implementation_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact pair (its own precondition does not require a plan)");
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("a missing stored planning result must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

#[test]
fn save_context_package_implementation_transition_rejects_a_non_completed_stored_planning_result() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, fixture.project_id);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    let expected_version = task.version();
    // A `Cancelled` outcome pairs with a NULL `plan_text` under the
    // `task_planning_results` CHECK constraint (0007_task_planning_results.sql)
    // -- unlike a `Completed`-with-empty-`plan_text` row, which that same
    // CHECK constraint makes structurally impossible to insert at all, so it
    // is not exercised as a separate test here.
    fixture
        .database
        .open_raw()
        .execute(
            "INSERT INTO task_planning_results (
                task_id, provider, work_kind, outcome, exit_code, turn_count,
                started_at_ms, completed_at_ms, plan_text
             ) VALUES (?1, 'Claude', 'Planning', 'Cancelled', NULL, NULL, 135, 150, NULL)",
            params![task.id().to_string()],
        )
        .expect("seed a non-completed stored planning result");
    repository
        .prepare_implementation_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact pair (its own precondition does not require a plan)");
    let before = task.clone();
    let history_before = repository
        .list_task_transitions(task.id())
        .expect("history before");

    let record = transition_to_implementing(&mut task, 210);

    assert_code(
        repository
            .save_context_package_implementation_transition(expected_version, &task, &record)
            .expect_err("a non-Completed stored planning result must be fail-closed"),
        RepositoryErrorCode::InvalidPersistenceState,
    );

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    assert_eq!(
        repository
            .list_task_transitions(before.id())
            .expect("history unchanged"),
        history_before
    );
}

fn digest_of(byte: u8) -> ManualResolutionDigest {
    ManualResolutionDigest::from_digest_bytes([byte; 32])
}

#[test]
fn ensure_manual_merge_resolution_confirmation_first_call_creates_the_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();

    let created = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            300,
        )
        .expect("first call must create the confirmation");
    assert_eq!(
        created,
        ManualMergeResolutionConfirmationRecord {
            task_id: task.id(),
            merge_conflict_task_version: expected_version,
            source_approval_task_version: 0,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            resolution_digest: digest_of(1),
            confirmed_at_ms: 300,
        }
    );
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_manual_merge_resolution_confirmations"
        ),
        1
    );
}

#[test]
fn ensure_manual_merge_resolution_confirmation_reuses_the_exact_identity_idempotently() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();

    let first = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            300,
        )
        .expect("first call creates the confirmation");
    let second = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            999,
        )
        .expect("second call reuses the existing confirmation");

    assert_eq!(first, second);
    assert_eq!(
        second.confirmed_at_ms, 300,
        "the original timestamp must be preserved"
    );
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_manual_merge_resolution_confirmations"
        ),
        1
    );
}

#[test]
fn ensure_manual_merge_resolution_confirmation_for_different_digests_are_independent() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();

    let first = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            300,
        )
        .expect("create the first digest's confirmation");
    let second = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"c".repeat(40),
            &"c".repeat(40),
            digest_of(2),
            310,
        )
        .expect("create the independent second digest's confirmation");

    assert_ne!(first, second);
    assert_eq!(
        repository
            .get_manual_merge_resolution_confirmation(task.id(), expected_version, digest_of(1))
            .expect("read the first digest back"),
        Some(first)
    );
    assert_eq!(
        repository
            .get_manual_merge_resolution_confirmation(task.id(), expected_version, digest_of(2))
            .expect("read the second digest back"),
        Some(second)
    );
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_manual_merge_resolution_confirmations"
        ),
        2
    );
}

#[test]
fn ensure_manual_merge_resolution_confirmation_rejects_a_task_not_in_merge_conflict() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let error = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            300,
        )
        .expect_err("a task not in MergeConflict must be rejected");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_manual_merge_resolution_confirmations"
        ),
        0
    );
}

#[test]
fn ensure_manual_merge_resolution_confirmation_rejects_a_stale_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let stale_version = task.version() - 1;

    let error = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            stale_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            300,
        )
        .expect_err("a stale expected version must be rejected");
    assert_code(error, RepositoryErrorCode::VersionConflict);
    assert_eq!(
        count_rows(
            &fixture.database.open_raw(),
            "task_manual_merge_resolution_confirmations"
        ),
        0
    );
}

#[test]
fn get_manual_merge_resolution_confirmation_fails_closed_on_a_corrupted_persisted_digest() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    drop(connection);

    let raw = fixture.database.open_raw();
    raw.execute_batch("PRAGMA ignore_check_constraints = 1;")
        .expect("disable CHECK enforcement for this fixture connection only");
    raw.execute(
        "INSERT INTO task_manual_merge_resolution_confirmations (
            task_id, merge_conflict_task_version, source_approval_task_version,
            base_commit_hex, task_commit_hex, merge_head_hex,
            resolution_digest_hex, confirmed_at_ms
         ) VALUES (?1, ?2, 0, ?3, ?4, ?4, 'not-a-valid-hex-digest', 100)",
        params![
            task.id().to_string(),
            expected_version as i64,
            "a".repeat(40),
            "b".repeat(40),
        ],
    )
    .expect("insert a row the CHECK constraint would normally reject");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .get_manual_merge_resolution_confirmation(task.id(), expected_version, digest_of(1))
        .expect_err("a malformed persisted digest must fail the lookup closed");
    assert_code(error, RepositoryErrorCode::InvalidPersistenceState);
}

#[test]
fn save_manual_merge_resolution_transition_commits_merging_once_confirmed() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest_of(1),
            300,
        )
        .expect("record the confirmation ahead of the transition");
    let lease_before = repository.active_lease().expect("read lease before");

    let previous_state = task.state();
    task.transition_to(TaskState::Merging, 310)
        .expect("domain transition");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 310);
    repository
        .save_manual_merge_resolution_transition(expected_version, &task, &record, digest_of(1))
        .expect("the exact confirmed digest must allow the transition to commit");

    let persisted = repository
        .get_task(task.id())
        .expect("read task after")
        .expect("task still exists");
    assert_eq!(persisted.state(), TaskState::Merging);
    assert_eq!(persisted.version(), expected_version + 1);
    let lease_after = repository.active_lease().expect("read lease after");
    assert_eq!(
        lease_before.map(|lease| lease.task_id),
        lease_after.map(|lease| lease.task_id),
        "Merging is non-terminal and must keep the active lease"
    );
}

#[test]
fn save_manual_merge_resolution_transition_rejects_a_missing_confirmation_without_writing() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let previous_state = task.state();
    task.transition_to(TaskState::Merging, 310)
        .expect("domain transition");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 310);
    let error = repository
        .save_manual_merge_resolution_transition(expected_version, &task, &record, digest_of(1))
        .expect_err("no confirmation exists for this exact identity yet");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before)
    );
}

#[test]
fn save_manual_merge_resolution_transition_rejects_a_task_not_in_merge_conflict() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_user_diff_approval(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let previous_state = task.state();
    task.transition_to(TaskState::Merging, 310)
        .expect("domain transition");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 310);
    let error = repository
        .save_manual_merge_resolution_transition(expected_version, &task, &record, digest_of(1))
        .expect_err("a task that was never in MergeConflict must be rejected");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before)
    );
}

#[test]
fn ensure_merge_abort_approval_first_call_creates_the_row() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();

    let created = repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            300,
        )
        .expect("first call must create the approval");
    assert_eq!(
        created,
        MergeAbortApprovalRecord {
            task_id: task.id(),
            merge_conflict_task_version: expected_version,
            source_approval_task_version: 0,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            approved_at_ms: 300,
        }
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_merge_abort_approvals"),
        1
    );
}

#[test]
fn ensure_merge_abort_approval_reuses_the_exact_identity_idempotently() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();

    let first = repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            300,
        )
        .expect("first call creates the approval");
    let second = repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            999,
        )
        .expect("second call reuses the existing approval");

    assert_eq!(first, second);
    assert_eq!(
        second.approved_at_ms, 300,
        "the original timestamp must be preserved"
    );
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_merge_abort_approvals"),
        1
    );
}

#[test]
fn ensure_merge_abort_approval_rejects_a_mismatched_commit_identity_for_the_same_task_version() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            300,
        )
        .expect("create the first approval");

    // Unlike `task_manual_merge_resolution_confirmations` (keyed by digest,
    // so a different digest is always an independent row), a merge-abort
    // approval's identity is `(task_id, merge_conflict_task_version)` alone
    // -- a different commit identity for that same pair can never be a
    // second row, and must not silently reuse the first row's stored
    // values either.
    let error = repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"c".repeat(40),
            &"c".repeat(40),
            310,
        )
        .expect_err("a different commit identity for the same (task, version) must be rejected");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_merge_abort_approvals"),
        1
    );
}

#[test]
fn ensure_merge_abort_approval_rejects_a_task_not_in_merge_conflict() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (task, _) = create_task(&mut repository, fixture.project_id);
    let expected_version = task.version();

    let error = repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            300,
        )
        .expect_err("a task not in MergeConflict must be rejected");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_merge_abort_approvals"),
        0
    );
}

#[test]
fn ensure_merge_abort_approval_rejects_a_stale_version_without_writing_anything() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let stale_version = task.version() - 1;

    let error = repository
        .ensure_merge_abort_approval(
            task.id(),
            stale_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            300,
        )
        .expect_err("a stale expected version must be rejected");
    assert_code(error, RepositoryErrorCode::VersionConflict);
    assert_eq!(
        count_rows(&fixture.database.open_raw(), "task_merge_abort_approvals"),
        0
    );
}

#[test]
fn get_merge_abort_approval_fails_closed_on_a_corrupted_persisted_commit() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    drop(connection);

    let raw = fixture.database.open_raw();
    raw.execute_batch("PRAGMA ignore_check_constraints = 1;")
        .expect("disable CHECK enforcement for this fixture connection only");
    raw.execute(
        "INSERT INTO task_merge_abort_approvals (
            task_id, merge_conflict_task_version, source_approval_task_version,
            base_commit_hex, task_commit_hex, merge_head_hex, approved_at_ms
         ) VALUES (?1, ?2, 0, ?3, 'not-a-valid-hex-commit', 'not-a-valid-hex-commit', 100)",
        params![
            task.id().to_string(),
            expected_version as i64,
            "a".repeat(40),
        ],
    )
    .expect("insert a row the CHECK constraint would normally reject");
    drop(raw);

    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let error = repository
        .get_merge_abort_approval(task.id(), expected_version)
        .expect_err("a malformed persisted commit must fail the lookup closed");
    assert_code(error, RepositoryErrorCode::InvalidPersistenceState);
}

#[test]
fn save_merge_abort_transition_commits_cancelled_and_releases_the_lease_once_approved() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    repository
        .ensure_merge_abort_approval(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            300,
        )
        .expect("record the approval ahead of the transition");
    let lease_before = repository.active_lease().expect("read lease before");
    assert_eq!(lease_before.map(|lease| lease.task_id), Some(task.id()));

    let previous_state = task.state();
    task.transition_to(TaskState::Cancelled, 310)
        .expect("domain transition");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 310);
    repository
        .save_merge_abort_transition(expected_version, &task, &record, true)
        .expect("an existing approval must allow the transition to commit");

    let persisted = repository
        .get_task(task.id())
        .expect("read task after")
        .expect("task still exists");
    assert_eq!(persisted.state(), TaskState::Cancelled);
    assert_eq!(persisted.version(), expected_version + 1);
    let lease_after = repository.active_lease().expect("read lease after");
    assert_eq!(
        lease_after, None,
        "Cancelled is terminal and must release the active lease in the same transaction"
    );
}

#[test]
fn save_merge_abort_transition_rejects_a_missing_approval_without_writing() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let previous_state = task.state();
    task.transition_to(TaskState::Cancelled, 310)
        .expect("domain transition");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 310);
    let error = repository
        .save_merge_abort_transition(expected_version, &task, &record, true)
        .expect_err("no approval exists for this exact identity yet");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before.clone())
    );
    let lease_after = repository.active_lease().expect("read lease after");
    assert_eq!(
        lease_after.map(|lease| lease.task_id),
        Some(before.id()),
        "a rejected transition must not release the lease"
    );
}

#[test]
fn save_merge_abort_transition_rejects_a_task_not_in_merge_conflict() {
    let fixture = Fixture::new();
    let mut connection = fixture.open();
    let mut repository = SqliteFoundationRepository::new(&mut connection);
    let (mut task, _) = create_task(&mut repository, fixture.project_id);
    advance_to_awaiting_user_diff_approval(&mut repository, &mut task);
    let expected_version = task.version();
    let before = task.clone();

    let previous_state = task.state();
    task.transition_to(TaskState::Cancelled, 310)
        .expect("domain transition");
    let record = transition(TaskStateTransitionId::new(), &task, previous_state, 310);
    let error = repository
        .save_merge_abort_transition(expected_version, &task, &record, true)
        .expect_err("a task that was never in MergeConflict must be rejected");
    assert_code(error, RepositoryErrorCode::InvalidAggregate);

    assert_eq!(
        repository.get_task(before.id()).expect("task unchanged"),
        Some(before)
    );
}

#[test]
fn two_connections_ensuring_the_same_merge_abort_approval_concurrently_both_succeed_with_exactly_one_row()
 {
    let fixture = Fixture::new();
    let (task, expected_version) = {
        let mut connection = fixture.open();
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        let (mut task, _) = create_task(&mut repository, fixture.project_id);
        advance_to_merge_conflict(&mut repository, &mut task);
        let expected_version = task.version();
        (task, expected_version)
    };

    let path = Arc::new(fixture.database.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for index in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        let task_id = task.id();
        handles.push(thread::spawn(move || {
            let mut database = DatabaseConnection::open(path.as_ref()).expect("open connection");
            let mut repository = SqliteFoundationRepository::new(&mut database);
            barrier.wait();
            let result = repository
                .ensure_merge_abort_approval(
                    task_id,
                    expected_version,
                    0,
                    &"a".repeat(40),
                    &"b".repeat(40),
                    &"b".repeat(40),
                    // Distinct timestamps prove whichever thread loses the race reuses the
                    // winner's persisted value instead of its own.
                    200 + index as i64,
                )
                .map_err(|error| error.code());
            sender.send(result).expect("send ensure result");
        }));
    }
    drop(sender);

    let results = (0..2)
        .map(|_| {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrent ensure timed out")
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("ensure thread panicked");
    }

    let successes = results
        .iter()
        .filter(|result| result.is_ok())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        successes.len(),
        2,
        "an idempotent create-or-reuse must let both concurrent callers succeed: {results:?}"
    );
    assert_eq!(
        successes[0], successes[1],
        "both concurrent callers must observe the exact same persisted approval, proving no \
         duplicate PK failure was exposed as a distinct successful result"
    );

    let connection = fixture.database.open_raw();
    assert_eq!(count_rows(&connection, "task_merge_abort_approvals"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}
