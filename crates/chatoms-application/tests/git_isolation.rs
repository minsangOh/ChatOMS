mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    git_isolation::{GitIsolationService, IsolationBlocker},
    projects::{ProjectMutationService, RegisterProjectRequest},
};
use chatoms_ports::{
    error::PortFailure,
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositoryKind, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome, WorktreePathProvider,
    },
    repository::{FoundationRepository, GitIsolationStatus, GitOperationReceiptKind},
};
use support::{FakeRepository, FakeTime};

struct GitFake {
    inspection: ProjectInspection,
    status: RepositoryStatus,
    author: bool,
    outcome: WorktreeCreationOutcome,
    snapshot_oid: String,
    init_calls: usize,
    create_calls: usize,
}

impl GitService for GitFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        Ok(true)
    }
    fn inspect_project(&mut self, _input: &Path) -> Result<ProjectInspection, PortFailure> {
        Ok(self.inspection.clone())
    }
    fn repository_status(&mut self, _root: &Path) -> Result<RepositoryStatus, PortFailure> {
        Ok(self.status.clone())
    }
    fn validate_non_git_source(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Ok(())
    }
    fn validate_repository_source(
        &mut self,
        _root: &Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Ok(RepositorySafetyToken {
            info_attributes_digest: "safe".to_owned(),
            info_attributes_identity: "stable".to_owned(),
        })
    }
    fn initialize_repository(&mut self, _root: &Path) -> Result<(), PortFailure> {
        self.init_calls += 1;
        self.inspection.repository_kind = RepositoryKind::Git;
        self.inspection.git_common_dir = Some(self.inspection.canonical_root.join(".git"));
        Ok(())
    }
    fn has_commit_author(&mut self, _root: &Path) -> Result<bool, PortFailure> {
        Ok(self.author)
    }
    fn create_initial_snapshot(&mut self, _root: &Path) -> Result<String, PortFailure> {
        Ok(self.snapshot_oid.clone())
    }
    fn create_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        self.create_calls += 1;
        Ok(self.outcome)
    }
    fn verify_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
    ) -> Result<bool, PortFailure> {
        Ok(self.outcome == WorktreeCreationOutcome::Created)
    }
}

struct PathsFake {
    target: PathBuf,
    calls: usize,
}

struct IdentityGuardFake(DirectoryIdentity);

impl DirectoryIdentityGuard for IdentityGuardFake {
    fn identity(&self) -> &DirectoryIdentity {
        &self.0
    }
}

#[derive(Default)]
struct FilesystemFake;

impl FilesystemIdentityPort for FilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000001".to_owned(),
            file_id_hex: if path.ends_with(".git") {
                "00000000000000000000000000000002".to_owned()
            } else {
                "00000000000000000000000000000001".to_owned()
            },
        })
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Ok(Box::new(IdentityGuardFake(
            self.inspect_supported_directory(path)?,
        )))
    }
}

struct ReboundFilesystemFake;

impl FilesystemIdentityPort for ReboundFilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000001".to_owned(),
            file_id_hex: "00000000000000000000000000000009".to_owned(),
        })
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Ok(Box::new(IdentityGuardFake(
            self.inspect_supported_directory(path)?,
        )))
    }
}

#[derive(Clone, Copy)]
enum PostMutationFilesystemFailure {
    RootInspection,
    CommonInspection,
    LocalTreeVerification,
}

struct PostMutationFilesystemFake {
    failure: PostMutationFilesystemFailure,
    inspection_calls: usize,
    tree_verification_calls: usize,
}

impl FilesystemIdentityPort for PostMutationFilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        let call = self.inspection_calls;
        self.inspection_calls += 1;
        if matches!(self.failure, PostMutationFilesystemFailure::RootInspection) && call == 1
            || matches!(
                self.failure,
                PostMutationFilesystemFailure::CommonInspection
            ) && call == 2
        {
            return Err(PortFailure::new(
                chatoms_ports::error::FailureCategory::Unsupported,
            ));
        }
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000001".to_owned(),
            file_id_hex: if path.ends_with(".git") {
                "00000000000000000000000000000002".to_owned()
            } else {
                "00000000000000000000000000000001".to_owned()
            },
        })
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        let call = self.tree_verification_calls;
        self.tree_verification_calls += 1;
        if matches!(
            self.failure,
            PostMutationFilesystemFailure::LocalTreeVerification
        ) && call == 1
        {
            return Err(PortFailure::new(
                chatoms_ports::error::FailureCategory::Unsupported,
            ));
        }
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Ok(Box::new(IdentityGuardFake(
            self.inspect_supported_directory(path)?,
        )))
    }
}
impl WorktreePathProvider for PathsFake {
    fn prepare_worktree_path(
        &mut self,
        _project_id: chatoms_domain::ProjectId,
        _task_id: chatoms_domain::TaskId,
    ) -> Result<PathBuf, PortFailure> {
        self.calls += 1;
        Ok(self.target.clone())
    }
}

fn git_fake(root: &Path, kind: RepositoryKind, clean: bool) -> GitFake {
    let status = RepositoryStatus {
        clean,
        detached_head: false,
        current_branch: Some("main".to_owned()),
        head_commit: Some("a".repeat(40)),
    };
    GitFake {
        inspection: ProjectInspection {
            canonical_root: root.to_owned(),
            canonical_key: root.to_string_lossy().to_lowercase().replace('\\', "/"),
            display_path: "%USERPROFILE%\\project".to_owned(),
            suggested_name: "Project".to_owned(),
            confirmation_token: "confirmation".to_owned(),
            repository_kind: kind,
            repository_status: (kind == RepositoryKind::Git).then_some(status.clone()),
            git_common_dir: (kind == RepositoryKind::Git).then(|| root.join(".git")),
        },
        status,
        author: true,
        outcome: WorktreeCreationOutcome::Created,
        snapshot_oid: "a".repeat(40),
        init_calls: 0,
        create_calls: 0,
    }
}

fn register(
    repository: &mut FakeRepository,
    git: &mut GitFake,
    time: &mut FakeTime,
) -> chatoms_domain::ProjectId {
    let mut filesystem = FilesystemFake;
    let token = ProjectMutationService::new(repository, git, &mut filesystem, time)
        .inspect_candidate("C:/input")
        .expect("candidate")
        .confirmation_token;
    ProjectMutationService::new(repository, git, &mut filesystem, time)
        .register_project(RegisterProjectRequest {
            input_path: "C:/input".to_owned(),
            confirmation_token: token,
            name: None,
        })
        .expect("register")
        .id()
}

#[test]
fn registration_persists_only_confirmed_canonical_candidate_and_display_path() {
    let directory = PathBuf::from("C:/fixture-registration");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let stored = repository
        .project_records
        .get(&project_id)
        .expect("stored project");
    assert_eq!(stored.root_path, directory.to_string_lossy());
    assert_eq!(stored.display_path, "%USERPROFILE%\\project");
    assert_eq!(repository.calls, ["create_project"]);
}

#[test]
fn registered_root_identity_rebinding_blocks_task_creation() {
    let directory = PathBuf::from("C:/fixture-rebound-root");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut filesystem = ReboundFilesystemFake;
    let mut paths = PathsFake {
        target: directory.join("managed"),
        calls: 0,
    };
    let error = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect_err("rebound root must fail before task intent");
    assert_eq!(error.code().as_str(), "APP_CONFLICT");
    assert!(repository.tasks.is_empty());
}

#[test]
fn dirty_repository_blocks_before_managed_path_or_git_mutation() {
    let directory = PathBuf::from("C:/fixture-dirty");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, false);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("worktree"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("create task");
    let blocked = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task_worktree(task.task_id, task.task_version)
    .expect("blocked view");
    assert_eq!(blocked.blocker, Some(IsolationBlocker::DirtyRepository));
    assert_eq!(paths.calls, 0);
    assert_eq!(git.create_calls, 0);
}

#[test]
fn uncertain_partial_worktree_effect_is_recorded_as_recovery_required_without_cleanup() {
    let directory = PathBuf::from("C:/fixture-uncertain");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, true);
    git.outcome = WorktreeCreationOutcome::Uncertain;
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed").join("task"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("create task");
    let result = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task_worktree(task.task_id, task.task_version)
    .expect("recovery view");
    assert_eq!(
        result.task_state,
        chatoms_domain::TaskState::RecoveryRequired
    );
    assert_eq!(
        result.isolation_status,
        GitIsolationStatus::RecoveryRequired
    );
}

#[test]
fn approved_init_binds_intent_and_missing_author_enters_recovery_after_mutation() {
    let directory = PathBuf::from("C:/fixture-init");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::NonGit, true);
    git.author = false;
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("awaiting approval");
    let result = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .approve_git_initialization(task.task_id, task.task_version)
    .expect("recovery view");
    assert_eq!(git.init_calls, 1);
    assert_eq!(repository.approvals.len(), 1);
    assert_eq!(
        repository.approvals[0].approved_task_version,
        task.task_version
    );
    assert_eq!(result.blocker, Some(IsolationBlocker::GitAuthorMissing));
    assert_eq!(
        result.task_state,
        chatoms_domain::TaskState::RecoveryRequired
    );
    let stale = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .approve_git_initialization(task.task_id, task.task_version)
    .expect_err("stale approval version cannot be replayed");
    assert_eq!(stale.code().as_str(), "APP_VERSION_CONFLICT");
}

#[test]
fn completed_git_effect_with_failed_completion_write_is_reconciled_to_recovery() {
    let directory = PathBuf::from("C:/fixture-init-persistence-failure");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::NonGit, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("awaiting approval");
    repository.fail_on = Some((
        "save_git_initialization_completion",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    let result = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .approve_git_initialization(task.task_id, task.task_version)
    .expect("recovery record after completion write failure");
    assert_eq!(
        result.task_state,
        chatoms_domain::TaskState::RecoveryRequired
    );
    assert_eq!(result.blocker, Some(IsolationBlocker::RecoveryRequired));
    assert_eq!(git.init_calls, 1);
}

#[test]
fn snapshot_oid_mismatch_never_records_git_initialized() {
    let directory = PathBuf::from("C:/fixture-snapshot-mismatch");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::NonGit, true);
    git.snapshot_oid = "b".repeat(40);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("awaiting approval");
    let result = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .approve_git_initialization(task.task_id, task.task_version)
    .expect("mismatch is durable recovery");
    assert_eq!(
        result.task_state,
        chatoms_domain::TaskState::RecoveryRequired
    );
    assert_eq!(
        repository
            .project_identities
            .get(&project_id)
            .map(|identity| identity.repository_kind),
        Some(RepositoryKind::NonGit)
    );
}

#[test]
fn post_mutation_filesystem_failures_immediately_preserve_recovery_evidence() {
    for failure in [
        PostMutationFilesystemFailure::RootInspection,
        PostMutationFilesystemFailure::CommonInspection,
        PostMutationFilesystemFailure::LocalTreeVerification,
    ] {
        let directory = PathBuf::from("C:/fixture-init-post-mutation-filesystem");
        let mut repository = FakeRepository::default();
        let mut git = git_fake(&directory, RepositoryKind::NonGit, true);
        let mut time = FakeTime::at(100);
        let project_id = register(&mut repository, &mut git, &mut time);
        let mut paths = PathsFake {
            target: directory.join("managed"),
            calls: 0,
        };
        let mut create_filesystem = FilesystemFake;
        let task = GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut create_filesystem,
            &mut paths,
            &mut time,
        )
        .create_task(project_id)
        .expect("awaiting approval");
        let mut filesystem = PostMutationFilesystemFake {
            failure,
            inspection_calls: 0,
            tree_verification_calls: 0,
        };
        let result = GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .approve_git_initialization(task.task_id, task.task_version)
        .expect("filesystem failure is durable recovery");

        assert_eq!(
            result.task_state,
            chatoms_domain::TaskState::RecoveryRequired
        );
        assert_eq!(
            result.isolation_status,
            GitIsolationStatus::RecoveryRequired
        );
        assert_eq!(
            repository.active_lease.map(|lease| lease.task_id),
            Some(task.task_id)
        );
        assert!(
            repository
                .receipts
                .iter()
                .any(|receipt| { receipt.kind == GitOperationReceiptKind::CommandSucceeded })
        );
        assert!(
            repository
                .receipts
                .iter()
                .any(|receipt| { receipt.kind == GitOperationReceiptKind::RecoveryRequired })
        );
        assert!(
            !repository
                .receipts
                .iter()
                .any(|receipt| { receipt.kind == GitOperationReceiptKind::CompletionRecorded })
        );
        assert_eq!(git.init_calls, 1);
    }
}

#[test]
fn recovery_write_failure_preserves_post_mutation_intent_and_receipt() {
    let directory = PathBuf::from("C:/fixture-init-recovery-write-failure");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::NonGit, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed"),
        calls: 0,
    };
    let mut create_filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut create_filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("awaiting approval");
    repository.fail_on = Some((
        "save_isolation_transition",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    let mut filesystem = PostMutationFilesystemFake {
        failure: PostMutationFilesystemFailure::RootInspection,
        inspection_calls: 0,
        tree_verification_calls: 0,
    };

    let error = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .approve_git_initialization(task.task_id, task.task_version)
    .expect_err("recovery persistence failure remains an error");

    assert_eq!(error.code().as_str(), "APP_INTERNAL");
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task.task_id)
    );
    assert_eq!(
        repository
            .isolations
            .get(&task.task_id)
            .map(|isolation| isolation.status),
        Some(GitIsolationStatus::GitInitInProgress)
    );
    assert!(
        repository
            .receipts
            .iter()
            .any(|receipt| { receipt.kind == GitOperationReceiptKind::CommandSucceeded })
    );
}

#[test]
fn startup_reconciliation_marks_incomplete_receipt_boundary_recovery_and_keeps_lease() {
    let directory = PathBuf::from("C:/fixture-startup-incomplete");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed").join("task"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("create task");
    repository.fail_on = Some((
        "append_git_operation_receipt",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task_worktree(task.task_id, task.task_version)
    .expect_err("simulated crash boundary before command start receipt");
    GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .reconcile_startup()
    .expect("startup reconciliation");
    let recovered = repository.tasks.get(&task.task_id).expect("recovered task");
    assert_eq!(
        recovered.state(),
        chatoms_domain::TaskState::RecoveryRequired
    );
    assert_eq!(
        repository.active_lease.as_ref().map(|lease| lease.task_id),
        Some(task.task_id)
    );
}

#[test]
fn startup_reconciliation_completes_only_exact_three_receipts_and_verified_state() {
    let directory = PathBuf::from("C:/fixture-startup-exact");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed").join("task"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("create task");
    repository.fail_on = Some((
        "append_git_operation_receipt",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task_worktree(task.task_id, task.task_version)
    .expect_err("pause at first receipt boundary");
    let operation_id = repository
        .isolations
        .get(&task.task_id)
        .and_then(|isolation| isolation.operation_id)
        .expect("persisted operation");
    for kind in [
        GitOperationReceiptKind::CommandStarted,
        GitOperationReceiptKind::CommandSucceeded,
        GitOperationReceiptKind::PostVerified,
    ] {
        repository
            .append_git_operation_receipt(operation_id, kind, None, 101)
            .expect("receipt");
    }
    GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .reconcile_startup()
    .expect("exact reconciliation");
    assert_eq!(
        repository.tasks.get(&task.task_id).map(|task| task.state()),
        Some(chatoms_domain::TaskState::WorktreeReady)
    );
    assert_eq!(
        repository.active_lease.as_ref().map(|lease| lease.task_id),
        Some(task.task_id)
    );
}

#[test]
fn startup_reconciliation_covers_every_worktree_receipt_crash_boundary() {
    let receipt_prefixes = [
        vec![],
        vec![GitOperationReceiptKind::CommandStarted],
        vec![
            GitOperationReceiptKind::CommandStarted,
            GitOperationReceiptKind::CommandSucceeded,
        ],
        vec![
            GitOperationReceiptKind::CommandStarted,
            GitOperationReceiptKind::CommandSucceeded,
            GitOperationReceiptKind::PostVerified,
        ],
    ];

    for (prefix_length, receipts) in receipt_prefixes.into_iter().enumerate() {
        let directory = PathBuf::from(format!(
            "C:/fixture-worktree-crash-boundary-{prefix_length}"
        ));
        let mut repository = FakeRepository::default();
        let mut git = git_fake(&directory, RepositoryKind::Git, true);
        let mut time = FakeTime::at(100);
        let project_id = register(&mut repository, &mut git, &mut time);
        let mut paths = PathsFake {
            target: directory.join("managed").join("task"),
            calls: 0,
        };
        let mut filesystem = FilesystemFake;
        let task = GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .create_task(project_id)
        .expect("create task");
        repository.fail_on = Some((
            "append_git_operation_receipt",
            chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
        ));
        GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .create_task_worktree(task.task_id, task.task_version)
        .expect_err("stop immediately after the durable intent");
        let operation_id = repository
            .isolations
            .get(&task.task_id)
            .and_then(|isolation| isolation.operation_id)
            .expect("persisted operation");
        for kind in receipts {
            repository
                .append_git_operation_receipt(operation_id, kind, None, 101)
                .expect("receipt prefix");
        }

        GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .reconcile_startup()
        .expect("startup reconciliation");

        let expected = if prefix_length == 3 {
            chatoms_domain::TaskState::WorktreeReady
        } else {
            chatoms_domain::TaskState::RecoveryRequired
        };
        assert_eq!(
            repository.tasks.get(&task.task_id).map(|task| task.state()),
            Some(expected),
            "receipt prefix length {prefix_length}"
        );
        assert_eq!(
            repository.active_lease.as_ref().map(|lease| lease.task_id),
            Some(task.task_id),
            "receipt prefix length {prefix_length}"
        );
    }
}

#[test]
fn startup_reconciliation_covers_every_git_init_receipt_crash_boundary() {
    let snapshot_oid = "a".repeat(40);
    let receipt_prefixes = [
        vec![],
        vec![GitOperationReceiptKind::CommandStarted],
        vec![
            GitOperationReceiptKind::CommandStarted,
            GitOperationReceiptKind::CommandSucceeded,
        ],
        vec![
            GitOperationReceiptKind::CommandStarted,
            GitOperationReceiptKind::CommandSucceeded,
            GitOperationReceiptKind::PostVerified,
        ],
    ];

    for (prefix_length, receipts) in receipt_prefixes.into_iter().enumerate() {
        let directory = PathBuf::from(format!("C:/fixture-init-crash-boundary-{prefix_length}"));
        let mut repository = FakeRepository::default();
        let mut git = git_fake(&directory, RepositoryKind::NonGit, true);
        let mut time = FakeTime::at(100);
        let project_id = register(&mut repository, &mut git, &mut time);
        let mut paths = PathsFake {
            target: directory.join("managed"),
            calls: 0,
        };
        let mut filesystem = FilesystemFake;
        let task = GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .create_task(project_id)
        .expect("awaiting approval");
        repository.fail_on = Some((
            "append_git_operation_receipt",
            chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
        ));
        GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .approve_git_initialization(task.task_id, task.task_version)
        .expect_err("stop immediately after the durable approval intent");
        let operation_id = repository
            .isolations
            .get(&task.task_id)
            .and_then(|isolation| isolation.operation_id)
            .expect("persisted operation");
        for kind in receipts {
            let evidence = (kind == GitOperationReceiptKind::CommandSucceeded)
                .then_some(snapshot_oid.as_str());
            repository
                .append_git_operation_receipt(operation_id, kind, evidence, 101)
                .expect("receipt prefix");
        }
        git.inspection.repository_kind = RepositoryKind::Git;
        git.inspection.git_common_dir = Some(directory.join(".git"));
        git.status.head_commit = Some(snapshot_oid.clone());

        GitIsolationService::new(
            &mut repository,
            &mut git,
            &mut filesystem,
            &mut paths,
            &mut time,
        )
        .reconcile_startup()
        .expect("startup reconciliation");

        let expected = if prefix_length == 3 {
            chatoms_domain::TaskState::GitInitialized
        } else {
            chatoms_domain::TaskState::RecoveryRequired
        };
        assert_eq!(
            repository.tasks.get(&task.task_id).map(|task| task.state()),
            Some(expected),
            "receipt prefix length {prefix_length}"
        );
        assert_eq!(
            repository.active_lease.as_ref().map(|lease| lease.task_id),
            Some(task.task_id),
            "receipt prefix length {prefix_length}"
        );
    }
}

#[test]
fn startup_reconciliation_fails_closed_on_lease_task_operation_mismatch() {
    let directory = PathBuf::from("C:/fixture-startup-corruption");
    let mut repository = FakeRepository::default();
    let mut git = git_fake(&directory, RepositoryKind::Git, true);
    let mut time = FakeTime::at(100);
    let project_id = register(&mut repository, &mut git, &mut time);
    let mut paths = PathsFake {
        target: directory.join("managed"),
        calls: 0,
    };
    let mut filesystem = FilesystemFake;
    let task_view = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .create_task(project_id)
    .expect("create task");
    let mut task = repository
        .tasks
        .get(&task_view.task_id)
        .expect("task")
        .clone();
    task.transition_to(chatoms_domain::TaskState::WorktreeCreating, 101)
        .expect("simulate corrupted persisted state");
    repository.tasks.insert(task.id(), task);
    let error = GitIsolationService::new(
        &mut repository,
        &mut git,
        &mut filesystem,
        &mut paths,
        &mut time,
    )
    .reconcile_startup()
    .expect_err("missing durable operation must fail closed");
    assert_eq!(error.code().as_str(), "APP_INTERNAL");
    assert_eq!(
        repository.active_lease.as_ref().map(|lease| lease.task_id),
        Some(task_view.task_id)
    );
}
