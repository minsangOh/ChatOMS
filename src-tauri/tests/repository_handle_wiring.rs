//! Regression coverage for a class of bug that fake-repository tests cannot
//! catch: a `FoundationRepository` method missing its `with_inner`/delegate
//! override in a wrapper falls back to the trait's default implementation
//! (`Err(OperationFailed)`) instead of ever reaching the real database.
//!
//! This exercises the exact production repository chain —
//! `RepositoryHandle` (src-tauri) wrapping `SharedFoundationRepository`
//! (chatoms-infrastructure) wrapping a real, migrated SQLite database — for
//! every provider-binding and Claude Planning method. `RepositoryFake`
//! (`src-tauri/src/commands/tests.rs`) and `FakeRepository`
//! (`chatoms-application/tests/support/mod.rs`) implement these methods
//! directly and so bypass both wrapper layers entirely; only a test built on
//! the real `SharedDatabase` composition, as here, can prove the delegation
//! itself is wired correctly. No Claude/Codex CLI is executed.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chatoms_app_lib::state::RepositoryHandle;
use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskId, TaskState, TaskStateTransition,
    TaskStateTransitionId, TaskStateTransitionSnapshot, ValidationCommandKind, WorkKind,
};
use chatoms_infrastructure::bootstrap::{DatabaseBootstrapAdapter, SharedDatabase};
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState,
    git::RepositoryKind,
    path::ResolvedAppPaths,
    provider::ProviderKind,
    repository::{
        AppProfileRecord, FoundationRepository, ImplementationResultOutcome, PlanningResultOutcome,
        ProjectFilesystemIdentityRecord, ProjectRecord, ProviderBindingRecord, ProviderConsent,
        ReviewResultOutcome, TaskImplementationResultRecord, TaskPlanningResultRecord,
        TaskReviewResultRecord, ValidationCommandApprovalRecord, ValidationCommandResultAttempt,
        ValidationCommandResultOutcome,
    },
};

/// A uniquely named directory under the OS temp root, cleaned up on drop.
/// Avoids adding a `tempfile` dev-dependency to `src-tauri/Cargo.toml`
/// (out of scope for this fix) while still giving each test its own SQLite
/// file; `ProjectId::new()` (a UUIDv7) is reused purely as a collision-free
/// name generator.
struct TempAppDir {
    root: std::path::PathBuf,
}

impl TempAppDir {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "chatoms-repository-handle-wiring-{label}-{}",
            ProjectId::new()
        ));
        std::fs::create_dir_all(root.join("data")).expect("create data directory");
        std::fs::create_dir_all(root.join("logs")).expect("create logs directory");
        Self { root }
    }

    fn paths(&self) -> ResolvedAppPaths {
        ResolvedAppPaths {
            app_root: self.root.clone(),
            data_dir: self.root.join("data"),
            database_path: self.root.join("data/chatoms.sqlite3"),
            logs_dir: self.root.join("logs"),
            artifacts_dir: self.root.join("artifacts"),
            temp_dir: self.root.join("temp"),
            worktrees_dir: self.root.join("worktrees"),
        }
    }
}

impl Drop for TempAppDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Migrates a fresh, real SQLite database and returns it wrapped exactly as
/// `production_runtime()` (`src-tauri/src/bootstrap.rs`) wraps it:
/// `RepositoryHandle::new(SharedFoundationRepository)`.
fn real_repository_handle(label: &str) -> (TempAppDir, RepositoryHandle) {
    let dir = TempAppDir::new(label);
    let shared_paths = Arc::new(Mutex::new(Some(dir.paths())));
    let database = SharedDatabase::default();
    let mut adapter = DatabaseBootstrapAdapter::new(shared_paths, database.clone());
    assert_eq!(
        adapter
            .bootstrap_database()
            .expect("migrate a real sqlite database"),
        DatabaseBootstrapState::Upgraded
    );
    (dir, RepositoryHandle::new(database.repository()))
}

fn project_record(project_id: ProjectId) -> ProjectRecord {
    ProjectRecord {
        id: project_id,
        name: "Foundation".to_owned(),
        root_path: "C:/repo".to_owned(),
        canonical_path_key: "c:/repo".to_owned(),
        display_path: "%USERPROFILE%\\repo".to_owned(),
        created_at_ms: 100,
        updated_at_ms: 100,
    }
}

/// `create_task` requires a *confirmed* filesystem identity row for the
/// owning project (a Phase 2 invariant enforced at the SQLite level), so a
/// plain `create_project` is not enough to seed a task in these tests.
fn confirmed_identity(project_id: ProjectId) -> ProjectFilesystemIdentityRecord {
    ProjectFilesystemIdentityRecord {
        project_id,
        root_volume_serial_hex: "0000000000000001".to_owned(),
        root_file_id_hex: "00000000000000000000000000000001".to_owned(),
        repository_kind: RepositoryKind::NonGit,
        git_common_volume_serial_hex: None,
        git_common_file_id_hex: None,
        confirmed: true,
        revision: 1,
        verified_at_ms: 100,
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

fn transition_record(
    task: &Task,
    from_state: TaskState,
    occurred_at_ms: i64,
) -> TaskStateTransition {
    TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
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

/// Advances `task` to `next` via a plain, non-Planning-specific
/// `save_transition` call — the same helper shape
/// `chatoms-infrastructure/tests/repository_transactions.rs` uses. Only the
/// task's *stored current state* matters to `save_planning_transition` and
/// `save_planning_result`, not how it got there, so this is sufficient to
/// reach `WorktreeReady` and `Planning` without standing up the full Git
/// isolation state machine.
fn advance(
    repository: &mut RepositoryHandle,
    task: &mut Task,
    next: TaskState,
    occurred_at_ms: i64,
) {
    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(next, occurred_at_ms)
        .expect("domain transition");
    let record = transition_record(task, previous_state, occurred_at_ms);
    repository
        .save_transition(expected_version, task, &record)
        .expect("save_transition through both wrapper layers");
}

#[test]
fn provider_binding_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("binding");

    let profile = AppProfileRecord {
        id: "profile-1".to_owned(),
        name: "Default".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: profile.id.clone(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    };

    let ensured = repository
        .ensure_default_profile_and_claude_binding(&profile, &binding)
        .expect(
            "ensure_default_profile_and_claude_binding must reach the real repository, \
             not the trait's OperationFailed default",
        );
    assert_eq!(ensured.provider_kind, ProviderKind::Claude);
    assert!(ensured.executable_path.is_none());

    let fetched = repository
        .get_claude_binding(&profile.name)
        .expect("get_claude_binding must reach the real repository")
        .expect("the binding just created must be persisted and readable back");
    assert_eq!(fetched.id, ensured.id);
    assert!(fetched.executable_path.is_none());

    repository
        .update_claude_executable_path(&fetched.id, Some("C:/trusted/claude.exe"), 2)
        .expect("update_claude_executable_path must reach the real repository");

    let updated = repository
        .get_claude_binding(&profile.name)
        .expect("get_claude_binding after update must reach the real repository")
        .expect("the binding still exists after updating its executable path");
    assert_eq!(
        updated.executable_path.as_deref(),
        Some("C:/trusted/claude.exe")
    );
}

#[test]
fn planning_consent_and_transition_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("planning-transition");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);

    let none_yet = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Planning,
            task.version(),
        )
        .expect(
            "get_provider_consent must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(none_yet, None, "no consent has been recorded yet");

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        approved_task_version: task.version(),
        consented_at_ms: 135,
    };
    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 140)
        .expect("WorktreeReady -> Planning is a valid domain transition");
    let planning_transition = transition_record(&task, previous_state, 140);
    repository
        .save_planning_transition(
            expected_version,
            &task,
            &planning_transition,
            Some(&consent),
        )
        .expect(
            "save_planning_transition must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let reloaded = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Planning,
            consent.approved_task_version,
        )
        .expect("get_provider_consent after save must reach the real repository")
        .expect("the consent just saved must be persisted and readable back");
    assert_eq!(reloaded.consented_at_ms, 135);
}

#[test]
fn implementation_consent_and_transition_delegation_reaches_real_sqlite_through_both_wrapper_layers()
 {
    let (_dir, mut repository) = real_repository_handle("implementation-transition");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );

    let none_yet = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Implementation,
            task.version(),
        )
        .expect(
            "get_provider_consent must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(none_yet, None, "no consent has been recorded yet");

    let consent = ProviderConsent {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        approved_task_version: task.version(),
        consented_at_ms: 155,
    };
    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 160)
        .expect("AwaitingDesignApproval -> Implementing is a valid domain transition");
    let implementation_transition = transition_record(&task, previous_state, 160);
    repository
        .save_implementation_transition(
            expected_version,
            &task,
            &implementation_transition,
            Some(&consent),
        )
        .expect(
            "save_implementation_transition must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let reloaded = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Implementation,
            consent.approved_task_version,
        )
        .expect("get_provider_consent after save must reach the real repository")
        .expect("the consent just saved must be persisted and readable back");
    assert_eq!(reloaded.consented_at_ms, 155);
}

#[test]
fn review_consent_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("review-consent");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    advance(&mut repository, &mut task, TaskState::Implementing, 160);
    advance(&mut repository, &mut task, TaskState::Testing, 170);
    advance(&mut repository, &mut task, TaskState::Reviewing, 180);
    let expected_version = task.version();

    let none_yet = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
        )
        .expect(
            "get_provider_consent must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(none_yet, None, "no consent has been recorded yet");

    let consent = repository
        .save_review_consent(expected_version, task.id(), 185)
        .expect(
            "save_review_consent must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(consent.consented_at_ms, 185);
    assert_eq!(consent.work_kind, WorkKind::Review);
    assert_eq!(consent.approved_task_version, expected_version);

    assert_eq!(
        repository.get_task(task.id()).expect("get_task"),
        Some(task.clone()),
        "save_review_consent must never change task state or version"
    );

    let reloaded = repository
        .get_provider_consent(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
        )
        .expect("get_provider_consent after save must reach the real repository")
        .expect("the consent just saved must be persisted and readable back");
    assert_eq!(reloaded, consent);

    let reused = repository
        .save_review_consent(expected_version, task.id(), 999)
        .expect("a second call must reuse the existing consent, not fail");
    assert_eq!(
        reused, consent,
        "reusing an existing same-version consent must return it unchanged"
    );
}

#[test]
fn planning_result_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("planning-result");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);

    assert_eq!(
        repository.get_task_planning_result(task.id()).expect(
            "get_task_planning_result must reach the real repository, not the trait's \
                 OperationFailed default"
        ),
        None,
        "no planning result has been recorded yet"
    );

    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval is a valid domain transition");
    let result_transition = transition_record(&task, previous_state, 150);
    let result = TaskPlanningResultRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        outcome: PlanningResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(3),
        started_at_ms: 135,
        completed_at_ms: 150,
        plan_text: Some("masked plan text".to_owned()),
    };
    repository
        .save_planning_result(expected_version, &task, &result_transition, &result, false)
        .expect(
            "save_planning_result must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let stored = repository
        .get_task_planning_result(task.id())
        .expect("get_task_planning_result after save must reach the real repository")
        .expect("the planning result just saved must be persisted and readable back");
    assert_eq!(stored.outcome, PlanningResultOutcome::Completed);
    assert_eq!(stored.plan_text.as_deref(), Some("masked plan text"));
    assert_eq!(stored.turn_count, Some(3));
}

#[test]
fn implementation_result_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("implementation-result");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    advance(&mut repository, &mut task, TaskState::Implementing, 160);

    assert_eq!(
        repository.get_task_implementation_result(task.id()).expect(
            "get_task_implementation_result must reach the real repository, not the trait's \
                 OperationFailed default"
        ),
        None,
        "no implementation result has been recorded yet"
    );

    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::Testing, 170)
        .expect("Implementing -> Testing is a valid domain transition");
    let result_transition = transition_record(&task, previous_state, 170);
    let result = TaskImplementationResultRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Implementation,
        outcome: ImplementationResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(4),
        started_at_ms: 155,
        completed_at_ms: 170,
    };
    repository
        .save_implementation_result(expected_version, &task, &result_transition, &result)
        .expect(
            "save_implementation_result must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let stored = repository
        .get_task_implementation_result(task.id())
        .expect("get_task_implementation_result after save must reach the real repository")
        .expect("the implementation result just saved must be persisted and readable back");
    assert_eq!(stored.outcome, ImplementationResultOutcome::Completed);
    assert_eq!(stored.turn_count, Some(4));
}

#[test]
fn review_result_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("review-result");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    advance(&mut repository, &mut task, TaskState::Implementing, 160);
    advance(&mut repository, &mut task, TaskState::Testing, 170);
    advance(&mut repository, &mut task, TaskState::Reviewing, 180);

    assert_eq!(
        repository.get_task_review_result(task.id()).expect(
            "get_task_review_result must reach the real repository, not the trait's \
                 OperationFailed default"
        ),
        None,
        "no review result has been recorded yet"
    );

    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingUserDiffApproval, 190)
        .expect("Reviewing -> AwaitingUserDiffApproval is a valid domain transition");
    let result_transition = transition_record(&task, previous_state, 190);
    let result = TaskReviewResultRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        outcome: ReviewResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(4),
        started_at_ms: 175,
        completed_at_ms: 190,
        review_text: Some("masked review text".to_owned()),
    };
    repository
        .save_review_result(expected_version, &task, &result_transition, &result, false)
        .expect(
            "save_review_result must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let stored = repository
        .get_task_review_result(task.id())
        .expect("get_task_review_result after save must reach the real repository")
        .expect("the review result just saved must be persisted and readable back");
    assert_eq!(stored.outcome, ReviewResultOutcome::Completed);
    assert_eq!(stored.review_text.as_deref(), Some("masked review text"));
    assert_eq!(stored.turn_count, Some(4));
}

#[test]
fn validation_command_approval_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("validation-command-approval");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    advance(&mut repository, &mut task, TaskState::Implementing, 160);
    advance(&mut repository, &mut task, TaskState::Testing, 170);

    assert_eq!(
        repository
            .list_validation_command_approvals(task.id(), task.version())
            .expect(
                "list_validation_command_approvals must reach the real repository, not the \
                 trait's OperationFailed default"
            ),
        Vec::new(),
        "no validation command has been approved yet"
    );

    let approval = ValidationCommandApprovalRecord {
        task_id: task.id(),
        approved_task_version: task.version(),
        kind: ValidationCommandKind::Test,
        executable: "cargo".to_owned(),
        arguments: vec!["test".to_owned(), "--workspace".to_owned()],
        approved_executable_path: "C:/tools/cargo/bin/cargo.exe".to_owned(),
        executable_volume_serial_hex: "0000000000000002".to_owned(),
        executable_file_id_hex: "00000000000000000000000000000002".to_owned(),
        tool_directory_path: "C:/tools/cargo/bin".to_owned(),
        tool_directory_volume_serial_hex: "0000000000000001".to_owned(),
        tool_directory_file_id_hex: "00000000000000000000000000000001".to_owned(),
        approved_cargo_home_path: Some("C:/tools/cargo-home".to_owned()),
        cargo_home_volume_serial_hex: Some("0000000000000003".to_owned()),
        cargo_home_file_id_hex: Some("00000000000000000000000000000003".to_owned()),
        approved_rustup_home_path: Some("C:/tools/rustup-home".to_owned()),
        rustup_home_volume_serial_hex: Some("0000000000000004".to_owned()),
        rustup_home_file_id_hex: Some("00000000000000000000000000000004".to_owned()),
        approved_at_ms: 175,
    };
    repository
        .save_validation_command_approval(&approval)
        .expect(
            "save_validation_command_approval must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let stored = repository
        .list_validation_command_approvals(task.id(), task.version())
        .expect("list_validation_command_approvals after save must reach the real repository");
    assert_eq!(stored, vec![approval]);
}

#[test]
fn validation_command_result_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("validation-command-result");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    advance(&mut repository, &mut task, TaskState::Implementing, 160);
    advance(&mut repository, &mut task, TaskState::Testing, 170);

    assert_eq!(
        repository
            .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
            .expect(
                "list_validation_command_results must reach the real repository, not the \
                 trait's OperationFailed default"
            ),
        Vec::new(),
        "no validation command has been run yet"
    );

    let approval = ValidationCommandApprovalRecord {
        task_id: task.id(),
        approved_task_version: task.version(),
        kind: ValidationCommandKind::Test,
        executable: "cargo".to_owned(),
        arguments: vec!["test".to_owned(), "--workspace".to_owned()],
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
        approved_at_ms: 175,
    };
    repository
        .save_validation_command_approval(&approval)
        .expect("seed the approval this result attempt is bound to");

    let attempt = ValidationCommandResultAttempt {
        task_id: task.id(),
        approved_task_version: task.version(),
        kind: ValidationCommandKind::Test,
        outcome: ValidationCommandResultOutcome::Success,
        exit_code: Some(0),
        safe_summary: "cargo test passed".to_owned(),
        started_at_ms: 180,
        completed_at_ms: 190,
    };
    let appended = repository
        .append_validation_command_result(&attempt)
        .expect(
            "append_validation_command_result must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(appended.attempt_sequence, 1);

    let stored = repository
        .list_validation_command_results(task.id(), task.version(), ValidationCommandKind::Test)
        .expect("list_validation_command_results after append must reach the real repository");
    assert_eq!(stored, vec![appended]);
}

#[test]
fn finalize_validation_command_batch_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("validation-command-batch-finalize");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance(&mut repository, &mut task, TaskState::ProjectValidated, 110);
    advance(&mut repository, &mut task, TaskState::WorktreeCreating, 120);
    advance(&mut repository, &mut task, TaskState::WorktreeReady, 130);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    advance(&mut repository, &mut task, TaskState::Implementing, 160);
    advance(&mut repository, &mut task, TaskState::Testing, 170);
    let expected_version = task.version();

    let approval = ValidationCommandApprovalRecord {
        task_id: task.id(),
        approved_task_version: expected_version,
        kind: ValidationCommandKind::Test,
        executable: "cargo".to_owned(),
        arguments: vec!["test".to_owned(), "--workspace".to_owned()],
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
        approved_at_ms: 175,
    };
    repository
        .save_validation_command_approval(&approval)
        .expect("seed the approval this final result attempt is bound to");

    let attempt = ValidationCommandResultAttempt {
        task_id: task.id(),
        approved_task_version: expected_version,
        kind: ValidationCommandKind::Test,
        outcome: ValidationCommandResultOutcome::Success,
        exit_code: Some(0),
        safe_summary: "cargo test passed".to_owned(),
        started_at_ms: 180,
        completed_at_ms: 190,
    };
    let previous_state = task.state();
    task.transition_to(TaskState::Reviewing, 200)
        .expect("Testing -> Reviewing");
    let record = transition_record(&task, previous_state, 200);

    repository
        .finalize_validation_command_batch(expected_version, &task, &record, &attempt)
        .expect(
            "finalize_validation_command_batch must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let reloaded = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Reviewing);
    let stored = repository
        .list_validation_command_results(task.id(), expected_version, ValidationCommandKind::Test)
        .expect("list_validation_command_results after finalize must reach the real repository");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].outcome, ValidationCommandResultOutcome::Success);
    assert_eq!(
        repository
            .active_lease()
            .expect("active_lease through both wrapper layers")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Reviewing is not terminal and must keep the lease"
    );
}
