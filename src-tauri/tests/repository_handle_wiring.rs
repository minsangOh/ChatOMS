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
    ActorKind, ContextDataScope, HighRiskCategory, OperationRiskKind, ProjectId, ReasonCode,
    TargetIdentityDigest, Task, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot, ValidationCommandKind, ValidationExecutionScope, WorkKind,
};
use chatoms_infrastructure::bootstrap::{DatabaseBootstrapAdapter, SharedDatabase};
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState,
    diff::DiffContentHash,
    git::RepositoryKind,
    manual_merge_resolution::ManualResolutionDigest,
    path::ResolvedAppPaths,
    provider::ProviderKind,
    repository::{
        AppProfileRecord, ContextPackageManifestRecord, ContextPackagePreparation,
        DiffApprovalRecord, FoundationRepository, GitIsolationStatus, HighRiskApprovalRecord,
        ImplementationResultOutcome, ManualMergeResolutionConfirmationRecord,
        MergeAbortApprovalRecord, OperationRiskDeclarationRecord, PlanningResultOutcome,
        PostMergeValidationResultAttempt, PostMergeValidationResultOutcome,
        ProjectFilesystemIdentityRecord, ProjectRecord, ProviderBindingRecord, ProviderConsent,
        ReviewResultOutcome, TaskGitIsolation, TaskImplementationResultRecord,
        TaskPlanningResultRecord, TaskReviewResultRecord, ValidationCommandApprovalRecord,
        ValidationCommandResultAttempt, ValidationCommandResultOutcome,
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
            ContextDataScope::LegacyPhase4,
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
        data_scope: ContextDataScope::LegacyPhase4,
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
            ContextDataScope::LegacyPhase4,
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
            ContextDataScope::LegacyPhase4,
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
        data_scope: ContextDataScope::LegacyPhase4,
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
            ContextDataScope::LegacyPhase4,
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
            ContextDataScope::LegacyPhase4,
        )
        .expect(
            "get_provider_consent must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(none_yet, None, "no consent has been recorded yet");

    let consent = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            185,
        )
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
            ContextDataScope::LegacyPhase4,
        )
        .expect("get_provider_consent after save must reach the real repository")
        .expect("the consent just saved must be persisted and readable back");
    assert_eq!(reloaded, consent);

    let reused = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::LegacyPhase4,
            999,
        )
        .expect("a second call must reuse the existing consent, not fail");
    assert_eq!(
        reused, consent,
        "reusing an existing same-version consent must return it unchanged"
    );
}

#[test]
fn context_package_manifest_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("context-package-manifest");
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

    let consent = repository
        .save_review_consent(
            expected_version,
            task.id(),
            ContextDataScope::ContextPackageV1,
            185,
        )
        .expect("save_review_consent must reach the real repository");

    assert_eq!(
        repository
            .get_context_package_manifest(
                task.id(),
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .expect(
                "get_context_package_manifest must reach the real repository, not the trait's \
                 OperationFailed default"
            ),
        None,
        "no manifest has been recorded yet"
    );

    let manifest = ContextPackageManifestRecord {
        task_id: task.id(),
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Review,
        approved_task_version: consent.approved_task_version,
        data_scope: ContextDataScope::ContextPackageV1,
        created_at_ms: 190,
    };
    repository.save_context_package_manifest(&manifest).expect(
        "save_context_package_manifest must reach the real repository, not the trait's \
             OperationFailed default",
    );

    let reloaded = repository
        .get_context_package_manifest(
            task.id(),
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
            ContextDataScope::ContextPackageV1,
        )
        .expect("get_context_package_manifest after save must reach the real repository")
        .expect("the manifest just saved must be persisted and readable back");
    assert_eq!(reloaded, manifest);
}

#[test]
fn high_risk_approval_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("high-risk-approval");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    let expected_version = task.version();

    assert_eq!(
        repository
            .get_high_risk_approval(task.id(), expected_version, HighRiskCategory::DataMigration,)
            .expect(
                "get_high_risk_approval must reach the real repository, not the trait's \
                 OperationFailed default"
            ),
        None,
        "no approval has been recorded yet"
    );

    let approval = HighRiskApprovalRecord {
        task_id: task.id(),
        approved_task_version: expected_version,
        risk_category: HighRiskCategory::DataMigration,
        approved_at_ms: 210,
    };
    repository.save_high_risk_approval(&approval).expect(
        "save_high_risk_approval must reach the real repository, not the trait's \
         OperationFailed default",
    );

    let reloaded = repository
        .get_high_risk_approval(task.id(), expected_version, HighRiskCategory::DataMigration)
        .expect("get_high_risk_approval after save must reach the real repository")
        .expect("the approval just saved must be persisted and readable back");
    assert_eq!(reloaded, approval);
}

#[test]
fn ensure_high_risk_approval_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("ensure-high-risk-approval");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    let expected_version = task.version();

    let created = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::ArchitectureChange,
            210,
        )
        .expect(
            "ensure_high_risk_approval must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(
        created,
        HighRiskApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            risk_category: HighRiskCategory::ArchitectureChange,
            approved_at_ms: 210,
        }
    );

    let reused = repository
        .ensure_high_risk_approval(
            task.id(),
            expected_version,
            HighRiskCategory::ArchitectureChange,
            999,
        )
        .expect("a second call through both wrapper layers must reuse, not fail or overwrite");
    assert_eq!(
        reused, created,
        "reuse through the real wrapper chain must return the original persisted approval"
    );
}

#[test]
fn diff_approval_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("diff-approval");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    let expected_version = task.version();
    let hash = DiffContentHash::from_digest_bytes([11u8; 32]);

    assert_eq!(
        repository
            .get_diff_approval(task.id(), expected_version, hash)
            .expect(
                "get_diff_approval must reach the real repository, not the trait's \
                 OperationFailed default"
            ),
        None,
        "no approval has been recorded yet"
    );

    let approval = DiffApprovalRecord {
        task_id: task.id(),
        approved_task_version: expected_version,
        diff_content_hash: hash,
        approved_at_ms: 210,
    };
    repository.save_diff_approval(&approval).expect(
        "save_diff_approval must reach the real repository, not the trait's OperationFailed \
         default",
    );

    let reloaded = repository
        .get_diff_approval(task.id(), expected_version, hash)
        .expect("get_diff_approval after save must reach the real repository")
        .expect("the approval just saved must be persisted and readable back");
    assert_eq!(reloaded, approval);
}

#[test]
fn ensure_diff_approval_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("ensure-diff-approval");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    let expected_version = task.version();
    let hash = DiffContentHash::from_digest_bytes([22u8; 32]);

    let created = repository
        .ensure_diff_approval(task.id(), expected_version, hash, 210)
        .expect(
            "ensure_diff_approval must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(
        created,
        DiffApprovalRecord {
            task_id: task.id(),
            approved_task_version: expected_version,
            diff_content_hash: hash,
            approved_at_ms: 210,
        }
    );

    let reused = repository
        .ensure_diff_approval(task.id(), expected_version, hash, 999)
        .expect("a second call through both wrapper layers must reuse, not fail or overwrite");
    assert_eq!(
        reused, created,
        "reuse through the real wrapper chain must return the original persisted approval"
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
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
        target_project_id: None,
        target_project_identity_revision: None,
        target_root_volume_serial_hex: None,
        target_root_file_id_hex: None,
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
    assert_eq!(stored, vec![approval.clone()]);
    let scoped = repository
        .list_validation_command_approvals_for_scope(
            task.id(),
            task.version(),
            chatoms_domain::ValidationExecutionScope::TaskWorktree,
        )
        .expect("scoped approval lookup must reach the real repository");
    assert_eq!(scoped, vec![approval]);
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
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
        target_project_id: None,
        target_project_identity_revision: None,
        target_root_volume_serial_hex: None,
        target_root_file_id_hex: None,
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
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
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
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
        target_project_id: None,
        target_project_identity_revision: None,
        target_root_volume_serial_hex: None,
        target_root_file_id_hex: None,
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
        execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
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

/// Advances a freshly created task through the real Git isolation state
/// machine to `WorktreeReady` (task state *and* a real `task_git_isolations`
/// row at the matching status), mirroring
/// `chatoms-infrastructure/tests/repository_transactions.rs`'s identically
/// named helper. `advance` (used by every other test in this file) only
/// bumps the task's own state via a plain `save_transition` and never
/// creates an isolation row, which `prepare_planning_context_package`
/// requires.
fn worktree_ready_task_with_real_isolation(
    repository: &mut RepositoryHandle,
    project_id: ProjectId,
) -> Task {
    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    task.transition_to(TaskState::ProjectValidated, 101)
        .expect("classify project");
    let classified = transition_record(&task, TaskState::Created, 101);
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
        .expect("create isolation task through both wrapper layers");

    let previous = task.state();
    task.transition_to(TaskState::WorktreeCreating, 102)
        .expect("worktree intent state");
    let worktree_transition = transition_record(&task, previous, 102);
    isolation.status = GitIsolationStatus::WorktreeCreating;
    isolation.operation_id = Some(chatoms_domain::GitOperationId::new());
    isolation.expected_task_version = task.version();
    isolation.base_branch = Some("main".to_owned());
    isolation.base_commit = Some("a".repeat(40));
    isolation.worktree_path = Some("C:/managed/project/task".to_owned());
    isolation.updated_at_ms = 102;
    repository
        .save_isolation_transition(1, &task, &worktree_transition, &isolation)
        .expect("worktree creating transition through both wrapper layers");

    let previous = task.state();
    task.transition_to(TaskState::WorktreeReady, 103)
        .expect("worktree ready state");
    let ready_transition = transition_record(&task, previous, 103);
    isolation.status = GitIsolationStatus::WorktreeReady;
    isolation.expected_task_version = task.version();
    isolation.branch_created_by_app = true;
    isolation.worktree_created_by_app = true;
    isolation.updated_at_ms = 103;
    repository
        .save_worktree_completion(2, &task, &ready_transition, &isolation)
        .expect("worktree ready completion through both wrapper layers");

    task
}

#[test]
fn operation_risk_declaration_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("operation-risk-declaration");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed owning project");
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, project_id);
    advance(&mut repository, &mut task, TaskState::Planning, 140);
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingDesignApproval,
        150,
    );
    let declaration = OperationRiskDeclarationRecord {
        task_id: task.id(),
        approved_task_version: task.version(),
        operation_kind: OperationRiskKind::ProviderImplementation,
        target_identity_digest: TargetIdentityDigest::from_digest_bytes([12; 32]),
        declared_at_ms: 200,
    };

    repository
        .declare_operation_risk(&declaration, &[])
        .expect("declaration must reach real repository");

    let stored = repository
        .get_operation_risk_declaration(
            task.id(),
            task.version(),
            OperationRiskKind::ProviderImplementation,
        )
        .expect("lookup must reach real repository")
        .expect("declaration exists");
    assert_eq!(stored.record, declaration);
    assert!(stored.risk_categories.is_empty());
}

#[test]
fn planning_context_package_preparation_delegation_reaches_real_sqlite_through_both_wrapper_layers()
{
    let (_dir, mut repository) = real_repository_handle("prepare-planning-context-package");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");
    let task = worktree_ready_task_with_real_isolation(&mut repository, project_id);

    let first: ContextPackagePreparation = repository
        .prepare_planning_context_package(task.version(), task.id(), 200)
        .expect(
            "prepare_planning_context_package must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(first.consent.work_kind, WorkKind::Planning);
    assert_eq!(first.consent.data_scope, ContextDataScope::ContextPackageV1);
    assert_eq!(first.manifest.work_kind, WorkKind::Planning);

    let second = repository
        .prepare_planning_context_package(task.version(), task.id(), 999)
        .expect("a second call must reuse the existing pair, not fail");
    assert_eq!(
        second, first,
        "reusing an existing pair must return it unchanged"
    );

    let unchanged = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task exists");
    assert_eq!(unchanged.state(), TaskState::WorktreeReady);
    assert_eq!(unchanged.version(), task.version());
}

#[test]
fn context_package_planning_transition_delegation_reaches_real_sqlite_through_both_wrapper_layers()
{
    let (_dir, mut repository) = real_repository_handle("save-context-package-planning-transition");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, project_id);
    let expected_version = task.version();
    repository
        .prepare_planning_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact ContextPackageV1 pair first");

    let previous_state = task.state();
    task.transition_to(TaskState::Planning, 210)
        .expect("WorktreeReady -> Planning");
    let record = transition_record(&task, previous_state, 210);

    repository
        .save_context_package_planning_transition(expected_version, &task, &record)
        .expect(
            "save_context_package_planning_transition must reach the real repository, not the \
             trait's OperationFailed default",
        );

    let reloaded = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Planning);
    assert_eq!(
        repository
            .active_lease()
            .expect("active_lease through both wrapper layers")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Planning is not terminal and must keep the lease"
    );
}

#[test]
fn context_package_implementation_transition_delegation_reaches_real_sqlite_through_both_wrapper_layers()
 {
    let (_dir, mut repository) =
        real_repository_handle("save-context-package-implementation-transition");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");
    let mut task = worktree_ready_task_with_real_isolation(&mut repository, project_id);
    advance(&mut repository, &mut task, TaskState::Planning, 140);

    let plan_expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::AwaitingDesignApproval, 150)
        .expect("Planning -> AwaitingDesignApproval");
    let plan_transition = transition_record(&task, previous_state, 150);
    let plan_result = TaskPlanningResultRecord {
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
        .save_planning_result(
            plan_expected_version,
            &task,
            &plan_transition,
            &plan_result,
            false,
        )
        .expect("save_planning_result must reach the real repository");

    let expected_version = task.version();
    repository
        .prepare_implementation_context_package(expected_version, task.id(), 200)
        .expect("prepare the exact ContextPackageV1 pair first");

    let previous_state = task.state();
    task.transition_to(TaskState::Implementing, 210)
        .expect("AwaitingDesignApproval -> Implementing");
    let record = transition_record(&task, previous_state, 210);

    repository
        .save_context_package_implementation_transition(expected_version, &task, &record)
        .expect(
            "save_context_package_implementation_transition must reach the real repository, not \
             the trait's OperationFailed default",
        );

    let reloaded = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Implementing);
    assert_eq!(
        repository
            .active_lease()
            .expect("active_lease through both wrapper layers")
            .map(|lease| lease.task_id),
        Some(task.id()),
        "Implementing is not terminal and must keep the lease"
    );
}

#[test]
fn implementation_context_package_preparation_delegation_reaches_real_sqlite_through_both_wrapper_layers()
 {
    let (_dir, mut repository) = real_repository_handle("prepare-implementation-context-package");
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

    let first: ContextPackagePreparation = repository
        .prepare_implementation_context_package(task.version(), task.id(), 200)
        .expect(
            "prepare_implementation_context_package must reach the real repository, not the \
             trait's OperationFailed default",
        );
    assert_eq!(first.consent.work_kind, WorkKind::Implementation);
    assert_eq!(first.consent.data_scope, ContextDataScope::ContextPackageV1);

    let second = repository
        .prepare_implementation_context_package(task.version(), task.id(), 999)
        .expect("a second call must reuse the existing pair, not fail");
    assert_eq!(
        second, first,
        "reusing an existing pair must return it unchanged"
    );
}

#[test]
fn review_context_package_preparation_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("prepare-review-context-package");
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

    let first: ContextPackagePreparation = repository
        .prepare_review_context_package(task.version(), task.id(), 200)
        .expect(
            "prepare_review_context_package must reach the real repository, not the trait's \
             OperationFailed default",
        );
    assert_eq!(first.consent.work_kind, WorkKind::Review);
    assert_eq!(first.consent.data_scope, ContextDataScope::ContextPackageV1);

    let second = repository
        .prepare_review_context_package(task.version(), task.id(), 999)
        .expect("a second call must reuse the existing pair, not fail");
    assert_eq!(
        second, first,
        "reusing an existing pair must return it unchanged"
    );
}

#[test]
fn post_merge_validation_delegation_reaches_real_sqlite_through_both_wrapper_layers() {
    let (_dir, mut repository) = real_repository_handle("post-merge-validation");
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
    advance(
        &mut repository,
        &mut task,
        TaskState::AwaitingUserDiffApproval,
        190,
    );

    let approval_task_version = task.version();
    repository
        .save_validation_command_approval(&ValidationCommandApprovalRecord {
            task_id: task.id(),
            approved_task_version: approval_task_version,
            execution_scope: ValidationExecutionScope::ProjectRoot,
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
            target_project_id: Some(project_id),
            target_project_identity_revision: Some(1),
            target_root_volume_serial_hex: Some("0000000000000001".to_owned()),
            target_root_file_id_hex: Some("00000000000000000000000000000001".to_owned()),
            approved_at_ms: 195,
        })
        .expect("save ProjectRoot approval through both wrapper layers");

    advance(&mut repository, &mut task, TaskState::Merging, 200);
    advance(&mut repository, &mut task, TaskState::PostMergeTesting, 210);
    let post_merge_task_version = task.version();
    let attempt = PostMergeValidationResultAttempt {
        task_id: task.id(),
        approval_task_version,
        post_merge_task_version,
        execution_scope: ValidationExecutionScope::ProjectRoot,
        kind: ValidationCommandKind::Test,
        outcome: PostMergeValidationResultOutcome::Success,
        exit_code: Some(0),
        safe_summary: "approved post-merge test passed".to_owned(),
        started_at_ms: 220,
        completed_at_ms: 230,
    };
    let appended = repository
        .append_post_merge_validation_result(&attempt)
        .expect("append_post_merge_validation_result must reach the real repository");
    assert_eq!(appended.attempt_sequence, 1);
    assert_eq!(
        repository
            .list_post_merge_validation_results(
                task.id(),
                approval_task_version,
                post_merge_task_version,
                ValidationCommandKind::Test,
            )
            .expect("list_post_merge_validation_results must reach the real repository"),
        vec![appended],
    );

    let expected_version = task.version();
    let previous_state = task.state();
    task.transition_to(TaskState::Completed, 240)
        .expect("PostMergeTesting -> Completed");
    let transition = transition_record(&task, previous_state, 240);
    repository
        .finalize_post_merge_validation_batch(expected_version, &task, &transition, &attempt)
        .expect("finalize_post_merge_validation_batch must reach the real repository");

    let reloaded = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task exists");
    assert_eq!(reloaded.state(), TaskState::Completed);
    assert_eq!(
        repository
            .list_post_merge_validation_results(
                task.id(),
                approval_task_version,
                post_merge_task_version,
                ValidationCommandKind::Test,
            )
            .expect("final post-merge result must be readable through both wrappers")
            .len(),
        2,
    );
    assert_eq!(
        repository
            .active_lease()
            .expect("active_lease through both wrapper layers"),
        None,
        "Completed must release the active lease"
    );
}

/// Advances `task` from `Created` all the way to `MergeConflict` via plain
/// `save_transition` calls (mirroring `advance`'s "only the stored state
/// matters" reasoning) -- neither `ensure_manual_merge_resolution_confirmation`/
/// `save_manual_merge_resolution_transition` nor
/// `ensure_merge_abort_approval`/`save_merge_abort_transition` need a real
/// Git isolation record, only the task's current state and version.
fn advance_to_merge_conflict(repository: &mut RepositoryHandle, task: &mut Task) {
    advance(repository, task, TaskState::ProjectValidated, 110);
    advance(repository, task, TaskState::WorktreeCreating, 120);
    advance(repository, task, TaskState::WorktreeReady, 130);
    advance(repository, task, TaskState::Planning, 140);
    advance(repository, task, TaskState::AwaitingDesignApproval, 150);
    advance(repository, task, TaskState::Implementing, 160);
    advance(repository, task, TaskState::Testing, 170);
    advance(repository, task, TaskState::Reviewing, 180);
    advance(repository, task, TaskState::AwaitingUserDiffApproval, 190);
    advance(repository, task, TaskState::Merging, 200);
    advance(repository, task, TaskState::MergeConflict, 210);
}

#[test]
fn manual_merge_resolution_confirmation_and_transition_delegation_reaches_real_sqlite_through_both_wrapper_layers()
 {
    // Regression test for a delegation gap this Unit found: `RepositoryHandle`
    // (src-tauri/src/state.rs) had no `with_inner` overrides at all for
    // `get_manual_merge_resolution_confirmation`,
    // `ensure_manual_merge_resolution_confirmation`, or
    // `save_manual_merge_resolution_transition` -- every call silently fell
    // through to the `FoundationRepository` trait's `OperationFailed`
    // default instead of ever reaching `SharedFoundationRepository`, the
    // same class of bug this file's module doc describes for provider
    // binding and Claude Planning.
    let (_dir, mut repository) = real_repository_handle("manual-merge-resolution");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();
    let digest = ManualResolutionDigest::from_digest_bytes([33u8; 32]);

    assert_eq!(
        repository
            .get_manual_merge_resolution_confirmation(task.id(), expected_version, digest)
            .expect(
                "get_manual_merge_resolution_confirmation must reach the real repository, not \
                 the trait's OperationFailed default"
            ),
        None,
        "no confirmation has been recorded yet"
    );

    let created = repository
        .ensure_manual_merge_resolution_confirmation(
            task.id(),
            expected_version,
            0,
            &"a".repeat(40),
            &"b".repeat(40),
            &"b".repeat(40),
            digest,
            300,
        )
        .expect(
            "ensure_manual_merge_resolution_confirmation must reach the real repository, not \
             the trait's OperationFailed default",
        );
    assert_eq!(
        created,
        ManualMergeResolutionConfirmationRecord {
            task_id: task.id(),
            merge_conflict_task_version: expected_version,
            source_approval_task_version: 0,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            resolution_digest: digest,
            confirmed_at_ms: 300,
        }
    );

    let reloaded = repository
        .get_manual_merge_resolution_confirmation(task.id(), expected_version, digest)
        .expect(
            "get_manual_merge_resolution_confirmation after ensure must reach the real repository",
        )
        .expect("the confirmation just created must be persisted and readable back");
    assert_eq!(reloaded, created);

    let previous_state = task.state();
    task.transition_to(TaskState::Merging, 310)
        .expect("MergeConflict -> Merging is a valid domain transition");
    let record = transition_record(&task, previous_state, 310);
    repository
        .save_manual_merge_resolution_transition(expected_version, &task, &record, digest)
        .expect(
            "save_manual_merge_resolution_transition must reach the real repository, not the \
             trait's OperationFailed default",
        );

    let persisted = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task still exists");
    assert_eq!(persisted.state(), TaskState::Merging);
    assert_eq!(persisted.version(), expected_version + 1);
}

#[test]
fn merge_abort_approval_and_transition_delegation_reaches_real_sqlite_through_both_wrapper_layers()
{
    let (_dir, mut repository) = real_repository_handle("merge-abort");
    let project_id = ProjectId::new();
    repository
        .create_project_with_identity(&project_record(project_id), &confirmed_identity(project_id))
        .expect("seed the owning project");

    let mut task = Task::new(TaskId::new(), project_id, 100);
    let initial = initial_transition(&task);
    repository
        .create_task(&task, &initial, 100)
        .expect("create_task through both wrapper layers");
    advance_to_merge_conflict(&mut repository, &mut task);
    let expected_version = task.version();

    assert_eq!(
        repository
            .get_merge_abort_approval(task.id(), expected_version)
            .expect(
                "get_merge_abort_approval must reach the real repository, not the trait's \
                 OperationFailed default"
            ),
        None,
        "no approval has been recorded yet"
    );

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
        .expect(
            "ensure_merge_abort_approval must reach the real repository, not the trait's \
             OperationFailed default",
        );
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

    let reloaded = repository
        .get_merge_abort_approval(task.id(), expected_version)
        .expect("get_merge_abort_approval after ensure must reach the real repository")
        .expect("the approval just created must be persisted and readable back");
    assert_eq!(reloaded, created);

    let previous_state = task.state();
    task.transition_to(TaskState::Cancelled, 310)
        .expect("MergeConflict -> Cancelled is a valid domain transition");
    let record = transition_record(&task, previous_state, 310);
    repository
        .save_merge_abort_transition(expected_version, &task, &record, true)
        .expect(
            "save_merge_abort_transition must reach the real repository, not the trait's \
             OperationFailed default",
        );

    let persisted = repository
        .get_task(task.id())
        .expect("get_task through both wrapper layers")
        .expect("task still exists");
    assert_eq!(persisted.state(), TaskState::Cancelled);
    assert_eq!(persisted.version(), expected_version + 1);
    assert_eq!(
        repository
            .active_lease()
            .expect("active_lease through both wrapper layers"),
        None,
        "Cancelled must release the active lease"
    );
}
