use std::{path::Path, str::FromStr};

use chatoms_domain::{
    ActorKind, GitOperationId, ProjectId, ReasonCode, Task, TaskBrief, TaskId, TaskState,
    TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot,
};
use chatoms_ports::{
    TimeProvider,
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, FilesystemIdentityPort},
    git::{
        GitService, RepositoryKind, RepositoryStatus, WorktreeCreationOutcome, WorktreePathProvider,
    },
    repository::{
        FoundationRepository, GitInitApproval, GitIsolationStatus, GitOperationKind,
        GitOperationReceipt, GitOperationReceiptKind, ProjectFilesystemIdentityRecord,
        ProjectRecord, TaskBriefRecord, TaskGitIsolation,
    },
};

use crate::error::ApplicationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsolationBlocker {
    DirtyRepository,
    DetachedHead,
    UnbornRepository,
    MissingCurrentBranch,
    GitAuthorMissing,
    GitOperationFailed,
    RecoveryRequired,
}

/// Raw, unvalidated task brief input for [`GitIsolationService::create_task`].
/// Validation happens inside `create_task` via [`TaskBrief::new`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskBriefDraft {
    pub requirements: String,
    pub completion_criteria: String,
    pub prohibited_scope: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskIsolationView {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub task_state: TaskState,
    pub task_version: u64,
    pub isolation_status: GitIsolationStatus,
    pub branch_identity: String,
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
    pub blocker: Option<IsolationBlocker>,
}

pub struct GitIsolationService<'a, R, G, F, W, T> {
    repository: &'a mut R,
    git: &'a mut G,
    filesystem: &'a mut F,
    worktree_paths: &'a mut W,
    time: &'a mut T,
}

impl<'a, R, G, F, W, T> GitIsolationService<'a, R, G, F, W, T>
where
    R: FoundationRepository,
    G: GitService,
    F: FilesystemIdentityPort,
    W: WorktreePathProvider,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        git: &'a mut G,
        filesystem: &'a mut F,
        worktree_paths: &'a mut W,
        time: &'a mut T,
    ) -> Self {
        Self {
            repository,
            git,
            filesystem,
            worktree_paths,
            time,
        }
    }

    pub fn create_task(
        &mut self,
        project_id: ProjectId,
        brief: Option<TaskBriefDraft>,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let project = self.load_project(project_id)?;
        let identity = self.load_project_identity(project_id)?;
        let inspection = self
            .git
            .inspect_project(Path::new(&project.root_path))
            .map_err(port_error)?;
        if inspection.canonical_key != project.canonical_path_key
            || !self.identity_matches_inspection(&identity, &inspection)?
        {
            return Err(category_error(FailureCategory::Conflict));
        }
        let now = self.now()?;
        let task_id = TaskId::new();
        let mut task = Task::new(task_id, project_id, now);
        let initial = TaskStateTransition::initial(
            TaskStateTransitionId::new(),
            task_id,
            actor("application")?,
            reason("task.created")?,
            now,
        );
        let target = match inspection.repository_kind {
            RepositoryKind::Git => TaskState::ProjectValidated,
            RepositoryKind::NonGit => TaskState::AwaitingGitInitApproval,
        };
        task.transition_to(target, now).map_err(domain_error)?;
        let classified = transition_record(&task, 2, TaskState::Created, "project.classified")?;
        let isolation_status = match inspection.repository_kind {
            RepositoryKind::Git => GitIsolationStatus::Ready,
            RepositoryKind::NonGit => GitIsolationStatus::AwaitingGitInitApproval,
        };
        let isolation = TaskGitIsolation {
            task_id,
            project_id,
            status: isolation_status,
            operation_id: None,
            expected_task_version: task.version(),
            base_branch: None,
            base_commit: None,
            worktree_path: None,
            branch_created_by_app: false,
            worktree_created_by_app: false,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let brief_record = brief
            .map(|draft| {
                TaskBrief::new(
                    draft.requirements,
                    draft.completion_criteria,
                    draft.prohibited_scope,
                )
                .map_err(domain_error)
                .map(|brief| TaskBriefRecord {
                    task_id,
                    requirements: brief.requirements().to_owned(),
                    completion_criteria: brief.completion_criteria().to_owned(),
                    prohibited_scope: brief.prohibited_scope().to_owned(),
                    created_at_ms: now,
                })
            })
            .transpose()?;
        self.repository
            .create_isolation_task(
                &task,
                &initial,
                &classified,
                now,
                &isolation,
                brief_record.as_ref(),
            )
            .map_err(repository_error)?;
        Ok(view(&task, &isolation, None))
    }

    pub fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let task = self.load_task(task_id)?;
        let isolation = self.load_isolation(task_id)?;
        let blocker = (task.state() == TaskState::RecoveryRequired)
            .then_some(IsolationBlocker::RecoveryRequired);
        Ok(view(&task, &isolation, blocker))
    }

    pub fn reconcile_startup(&mut self) -> Result<(), ApplicationError> {
        let attempts = self
            .repository
            .list_incomplete_git_operations()
            .map_err(repository_error)?;
        if attempts.len() > 1 {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let startup_lease = self.repository.active_lease().map_err(repository_error)?;
        match startup_lease {
            Some(lease) => {
                let leased_task = self
                    .repository
                    .get_task(lease.task_id)
                    .map_err(repository_error)?
                    .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
                let isolation_in_progress = self
                    .repository
                    .get_task_isolation(lease.task_id)
                    .map_err(repository_error)?
                    .is_some_and(|isolation| {
                        matches!(
                            isolation.status,
                            GitIsolationStatus::GitInitInProgress
                                | GitIsolationStatus::WorktreeCreating
                        )
                    });
                let has_attempt = attempts
                    .first()
                    .is_some_and(|attempt| attempt.task_id == lease.task_id);
                if !leased_task.state().requires_active_lease()
                    || isolation_in_progress != has_attempt
                    || (leased_task.state() == TaskState::WorktreeCreating && !has_attempt)
                {
                    return Err(category_error(FailureCategory::InvariantViolation));
                }
            }
            None if !attempts.is_empty() => {
                return Err(category_error(FailureCategory::InvariantViolation));
            }
            None => {}
        }
        for attempt in attempts {
            let lease = self.repository.active_lease().map_err(repository_error)?;
            if lease.as_ref().map(|value| value.task_id) != Some(attempt.task_id) {
                return Err(category_error(FailureCategory::InvariantViolation));
            }
            let task = self.load_task(attempt.task_id)?;
            let isolation = self.load_isolation(attempt.task_id)?;
            let identity = self.load_project_identity(attempt.project_id)?;
            if task.project_id() != attempt.project_id
                || !task.state().requires_active_lease()
                || isolation.operation_id != Some(attempt.operation_id)
                || isolation.expected_task_version != task.version()
                || attempt.approved_task_version != task.version()
                || attempt.project_identity_revision != identity.revision
            {
                return Err(category_error(FailureCategory::InvariantViolation));
            }
            let receipts = self
                .repository
                .list_git_operation_receipts(attempt.operation_id)
                .map_err(repository_error)?;
            let exact_success = exact_success_receipts(&receipts);
            let reconciled = match attempt.operation_kind {
                GitOperationKind::GitInitialize if exact_success => self
                    .reconcile_completed_git_init(
                        task.clone(),
                        isolation.clone(),
                        &identity,
                        &receipts,
                    )?,
                GitOperationKind::WorktreeCreate if exact_success => {
                    self.reconcile_completed_worktree(task.clone(), isolation.clone(), &identity)?
                }
                _ => false,
            };
            if reconciled {
                continue;
            }
            self.mark_recovery(
                task,
                isolation,
                IsolationBlocker::RecoveryRequired,
                "git.startup.recovery-required",
            )?;
        }
        Ok(())
    }

    fn reconcile_completed_git_init(
        &mut self,
        mut task: Task,
        mut isolation: TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
        receipts: &[GitOperationReceipt],
    ) -> Result<bool, ApplicationError> {
        if task.state() != TaskState::AwaitingGitInitApproval
            || isolation.status != GitIsolationStatus::GitInitInProgress
            || identity.repository_kind != RepositoryKind::NonGit
        {
            return Ok(false);
        }
        let snapshot_oid = match receipts
            .get(1)
            .and_then(|receipt| receipt.evidence.as_deref())
        {
            Some(value) => value,
            None => return Ok(false),
        };
        let project = self.load_project(task.project_id())?;
        let inspection = match self.git.inspect_project(Path::new(&project.root_path)) {
            Ok(value) if value.repository_kind == RepositoryKind::Git => value,
            _ => return Ok(false),
        };
        let status = match self.git.repository_status(Path::new(&project.root_path)) {
            Ok(value)
                if value.ready_for_isolation()
                    && value.head_commit.as_deref() == Some(snapshot_oid) =>
            {
                value
            }
            _ => return Ok(false),
        };
        let _ = status;
        let actual_root = match self
            .filesystem
            .inspect_supported_directory(&inspection.canonical_root)
        {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };
        if actual_root.volume_serial_hex != identity.root_volume_serial_hex
            || actual_root.file_id_hex != identity.root_file_id_hex
        {
            return Ok(false);
        }
        let actual_common = match inspection
            .git_common_dir
            .as_deref()
            .map(|path| self.filesystem.inspect_supported_directory(path))
            .transpose()
        {
            Ok(Some(value)) => value,
            _ => return Ok(false),
        };
        let expected_version = task.version();
        let previous = task.state();
        task.transition_to(TaskState::GitInitialized, self.now()?)
            .map_err(domain_error)?;
        let transition = self.next_transition(&task, previous, "git.initialized.reconciled")?;
        isolation.status = GitIsolationStatus::Ready;
        isolation.expected_task_version = task.version();
        isolation.updated_at_ms = task.updated_at_ms();
        let updated_identity = ProjectFilesystemIdentityRecord {
            project_id: identity.project_id,
            root_volume_serial_hex: identity.root_volume_serial_hex.clone(),
            root_file_id_hex: identity.root_file_id_hex.clone(),
            repository_kind: RepositoryKind::Git,
            git_common_volume_serial_hex: Some(actual_common.volume_serial_hex),
            git_common_file_id_hex: Some(actual_common.file_id_hex),
            confirmed: true,
            revision: identity.revision.saturating_add(1),
            verified_at_ms: isolation.updated_at_ms,
        };
        self.repository
            .save_git_initialization_completion(
                expected_version,
                &task,
                &transition,
                &isolation,
                &updated_identity,
            )
            .map_err(repository_error)?;
        Ok(true)
    }

    fn reconcile_completed_worktree(
        &mut self,
        task: Task,
        isolation: TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<bool, ApplicationError> {
        if task.state() != TaskState::WorktreeCreating
            || isolation.status != GitIsolationStatus::WorktreeCreating
            || identity.repository_kind != RepositoryKind::Git
        {
            return Ok(false);
        }
        let project = self.load_project(task.project_id())?;
        let inspection = match self.git.inspect_project(Path::new(&project.root_path)) {
            Ok(value) if value.repository_kind == RepositoryKind::Git => value,
            _ => return Ok(false),
        };
        if !self.identity_matches_inspection(identity, &inspection)? {
            return Ok(false);
        }
        let (branch, base_commit, worktree) = match (
            isolation.base_branch.as_deref(),
            isolation.base_commit.as_deref(),
            isolation.worktree_path.as_deref(),
        ) {
            (Some(base_branch), Some(commit), Some(path)) => (
                task.task_branch_identity().as_str(),
                (base_branch, commit),
                Path::new(path),
            ),
            _ => return Ok(false),
        };
        let source = match self.git.repository_status(Path::new(&project.root_path)) {
            Ok(value) => value,
            Err(_) => return Ok(false),
        };
        if !source.clean
            || source.detached_head
            || source.current_branch.as_deref() != Some(base_commit.0)
            || source.head_commit.as_deref() != Some(base_commit.1)
            || !self
                .git
                .verify_task_worktree(
                    Path::new(&project.root_path),
                    branch,
                    base_commit.1,
                    worktree,
                )
                .unwrap_or(false)
            || self
                .filesystem
                .inspect_supported_directory(worktree)
                .and_then(|actual| self.filesystem.verify_local_tree(&actual.canonical_path))
                .is_err()
        {
            return Ok(false);
        }
        self.complete_worktree(task, isolation)?;
        Ok(true)
    }

    pub fn approve_git_initialization(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let mut task = self.load_expected_task(task_id, expected_version)?;
        let project = self.load_project(task.project_id())?;
        let identity = self.load_project_identity(task.project_id())?;
        let mut isolation = self.load_isolation(task_id)?;
        if task.state() != TaskState::AwaitingGitInitApproval
            || isolation.status != GitIsolationStatus::AwaitingGitInitApproval
            || isolation.expected_task_version != expected_version
        {
            return Err(category_error(FailureCategory::InvalidState));
        }
        if identity.repository_kind != RepositoryKind::NonGit || !identity.confirmed {
            return Err(category_error(FailureCategory::Conflict));
        }
        let root_identity = root_identity(&project, &identity);
        let _root_guard = self
            .filesystem
            .acquire_guard(Path::new(&project.root_path), &root_identity)
            .map_err(port_error)?;
        self.filesystem
            .verify_local_tree(Path::new(&project.root_path))
            .map_err(port_error)?;
        self.git
            .validate_non_git_source(Path::new(&project.root_path))
            .map_err(port_error)?;
        let now = self.now()?;
        let operation_id = GitOperationId::new();
        isolation.status = GitIsolationStatus::GitInitInProgress;
        isolation.operation_id = Some(operation_id);
        isolation.updated_at_ms = now;
        let approval = GitInitApproval {
            operation_id,
            task_id,
            project_id: task.project_id(),
            approved_task_version: expected_version,
            approved_at_ms: now,
        };
        self.repository
            .begin_git_initialization(expected_version, &isolation, &approval)
            .map_err(repository_error)?;
        self.record_receipt(operation_id, GitOperationReceiptKind::CommandStarted, None)?;

        if self
            .git
            .initialize_repository(Path::new(&project.root_path))
            .is_err()
        {
            return self.mark_recovery(
                task,
                isolation,
                IsolationBlocker::RecoveryRequired,
                "git.init.failed",
            );
        }
        match self.git.has_commit_author(Path::new(&project.root_path)) {
            Ok(true) => {}
            Ok(false) => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::GitAuthorMissing,
                    "git.author.missing",
                );
            }
            Err(_) => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::GitOperationFailed,
                    "git.author.check-failed",
                );
            }
        }
        let snapshot_oid = match self
            .git
            .create_initial_snapshot(Path::new(&project.root_path))
        {
            Ok(oid) => oid,
            Err(_) => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::GitOperationFailed,
                    "git.snapshot.failed",
                );
            }
        };
        self.record_receipt(
            operation_id,
            GitOperationReceiptKind::CommandSucceeded,
            Some(&snapshot_oid),
        )?;
        let status = match self.git.repository_status(Path::new(&project.root_path)) {
            Ok(status) if status.ready_for_isolation() => status,
            _ => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::GitOperationFailed,
                    "git.snapshot.unverified",
                );
            }
        };
        if status.head_commit.as_deref() != Some(snapshot_oid.as_str()) {
            return self.mark_recovery(
                task,
                isolation,
                IsolationBlocker::GitOperationFailed,
                "git.snapshot.oid-mismatch",
            );
        }
        let inspection = match self.git.inspect_project(Path::new(&project.root_path)) {
            Ok(value) if value.repository_kind == RepositoryKind::Git => value,
            _ => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::RecoveryRequired,
                    "git.init.identity-unverified",
                );
            }
        };
        let actual_root = match self
            .filesystem
            .inspect_supported_directory(&inspection.canonical_root)
        {
            Ok(identity) => identity,
            Err(_) => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::RecoveryRequired,
                    "git.init.root-inspection-failed",
                );
            }
        };
        let common_path = match inspection.git_common_dir.as_deref() {
            Some(path) => path,
            None => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::RecoveryRequired,
                    "git.init.common-directory-missing",
                );
            }
        };
        let actual_common = match self.filesystem.inspect_supported_directory(common_path) {
            Ok(identity) => identity,
            Err(_) => {
                return self.mark_recovery(
                    task,
                    isolation,
                    IsolationBlocker::RecoveryRequired,
                    "git.init.common-inspection-failed",
                );
            }
        };
        if !actual_root.same_object(&root_identity) {
            return self.mark_recovery(
                task,
                isolation,
                IsolationBlocker::RecoveryRequired,
                "git.init.root-identity-changed",
            );
        }
        if self
            .filesystem
            .verify_local_tree(&actual_root.canonical_path)
            .is_err()
        {
            return self.mark_recovery(
                task,
                isolation,
                IsolationBlocker::RecoveryRequired,
                "git.init.local-tree-unverified",
            );
        }
        self.record_receipt(operation_id, GitOperationReceiptKind::PostVerified, None)?;
        let previous = task.state();
        task.transition_to(TaskState::GitInitialized, self.now()?)
            .map_err(domain_error)?;
        let transition = self.next_transition(&task, previous, "git.initialized")?;
        isolation.status = GitIsolationStatus::Ready;
        isolation.expected_task_version = task.version();
        isolation.updated_at_ms = task.updated_at_ms();
        let updated_identity = ProjectFilesystemIdentityRecord {
            project_id: identity.project_id,
            root_volume_serial_hex: identity.root_volume_serial_hex,
            root_file_id_hex: identity.root_file_id_hex,
            repository_kind: RepositoryKind::Git,
            git_common_volume_serial_hex: Some(actual_common.volume_serial_hex),
            git_common_file_id_hex: Some(actual_common.file_id_hex),
            confirmed: true,
            revision: identity.revision.saturating_add(1),
            verified_at_ms: isolation.updated_at_ms,
        };
        if let Err(error) = self.repository.save_git_initialization_completion(
            expected_version,
            &task,
            &transition,
            &isolation,
            &updated_identity,
        ) {
            return self.recover_after_completion_write_failure(
                task_id,
                error,
                "git.initialized.persistence-failed",
            );
        }
        Ok(view(&task, &isolation, None))
    }

    pub fn create_task_worktree(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let mut task = self.load_expected_task(task_id, expected_version)?;
        if !matches!(
            task.state(),
            TaskState::ProjectValidated | TaskState::GitInitialized
        ) {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let project = self.load_project(task.project_id())?;
        let identity = self.load_project_identity(task.project_id())?;
        if identity.repository_kind != RepositoryKind::Git || !identity.confirmed {
            return Err(category_error(FailureCategory::Conflict));
        }
        let inspection = self
            .git
            .inspect_project(Path::new(&project.root_path))
            .map_err(port_error)?;
        if inspection.repository_kind != RepositoryKind::Git
            || inspection.canonical_key != project.canonical_path_key
            || !self.identity_matches_inspection(&identity, &inspection)?
        {
            return Err(category_error(FailureCategory::Conflict));
        }
        let root_expected = root_identity(&project, &identity);
        let common_path = inspection
            .git_common_dir
            .as_deref()
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let common_expected = common_identity(&identity, common_path)?;
        let _root_guard = self
            .filesystem
            .acquire_guard(Path::new(&project.root_path), &root_expected)
            .map_err(port_error)?;
        let _common_guard = self
            .filesystem
            .acquire_guard(common_path, &common_expected)
            .map_err(port_error)?;
        let mut isolation = self.load_isolation(task_id)?;
        let status = self
            .git
            .repository_status(Path::new(&project.root_path))
            .map_err(port_error)?;
        if let Some(blocker) = readiness_blocker(&status) {
            return Ok(view(&task, &isolation, Some(blocker)));
        }
        let base_branch = status
            .current_branch
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let base_commit = status
            .head_commit
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let safety = self
            .git
            .validate_repository_source(Path::new(&project.root_path), &base_commit)
            .map_err(port_error)?;
        let worktree = self
            .worktree_paths
            .prepare_worktree_path(task.project_id(), task.id())
            .map_err(port_error)?;
        let worktree_text = worktree
            .to_str()
            .ok_or_else(|| category_error(FailureCategory::InvalidInput))?
            .to_owned();
        let worktree_parent = worktree
            .parent()
            .ok_or_else(|| category_error(FailureCategory::InvalidInput))?;
        let parent_identity = self
            .filesystem
            .inspect_supported_directory(worktree_parent)
            .map_err(port_error)?;
        let _parent_guard = self
            .filesystem
            .acquire_guard(worktree_parent, &parent_identity)
            .map_err(port_error)?;
        let operation_id = GitOperationId::new();
        let previous = task.state();
        task.transition_to(TaskState::WorktreeCreating, self.now()?)
            .map_err(domain_error)?;
        let transition = self.next_transition(&task, previous, "git.worktree.intent")?;
        isolation.status = GitIsolationStatus::WorktreeCreating;
        isolation.operation_id = Some(operation_id);
        isolation.expected_task_version = task.version();
        isolation.base_branch = Some(base_branch);
        isolation.base_commit = Some(base_commit.clone());
        isolation.worktree_path = Some(worktree_text);
        isolation.branch_created_by_app = false;
        isolation.worktree_created_by_app = false;
        isolation.updated_at_ms = task.updated_at_ms();
        self.repository
            .save_isolation_transition(expected_version, &task, &transition, &isolation)
            .map_err(repository_error)?;

        let branch = task.task_branch_identity().as_str().to_owned();
        self.record_receipt(operation_id, GitOperationReceiptKind::CommandStarted, None)?;
        let create_result = self.git.create_task_worktree(
            Path::new(&project.root_path),
            &branch,
            &base_commit,
            &worktree,
            &safety,
        );
        if matches!(create_result, Ok(WorktreeCreationOutcome::Created)) {
            self.record_receipt(
                operation_id,
                GitOperationReceiptKind::CommandSucceeded,
                None,
            )?;
        }
        let verified = self
            .git
            .verify_task_worktree(
                Path::new(&project.root_path),
                &branch,
                &base_commit,
                &worktree,
            )
            .unwrap_or(false);
        let source_preserved = self
            .git
            .repository_status(Path::new(&project.root_path))
            .is_ok_and(|actual| {
                actual.clean
                    && !actual.detached_head
                    && actual.current_branch.as_deref()
                        == Some(isolation.base_branch.as_deref().unwrap_or_default())
                    && actual.head_commit.as_deref() == Some(base_commit.as_str())
            });
        let identities_preserved = self
            .git
            .inspect_project(Path::new(&project.root_path))
            .map_err(port_error)
            .and_then(|actual| self.identity_matches_inspection(&identity, &actual))
            .unwrap_or(false)
            && self
                .filesystem
                .inspect_supported_directory(worktree_parent)
                .is_ok_and(|actual| actual.same_object(&parent_identity));
        let worktree_identity_verified = self
            .filesystem
            .inspect_supported_directory(&worktree)
            .and_then(|actual| self.filesystem.verify_local_tree(&actual.canonical_path))
            .is_ok();
        if matches!(create_result, Ok(WorktreeCreationOutcome::Created))
            && verified
            && source_preserved
            && identities_preserved
            && worktree_identity_verified
        {
            self.record_receipt(operation_id, GitOperationReceiptKind::PostVerified, None)?;
            return self.complete_worktree(task, isolation);
        }
        self.mark_recovery(
            task,
            isolation,
            IsolationBlocker::RecoveryRequired,
            "git.worktree.failed-or-unverified",
        )
    }

    fn complete_worktree(
        &mut self,
        mut task: Task,
        mut isolation: TaskGitIsolation,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let expected_version = task.version();
        let previous = task.state();
        task.transition_to(TaskState::WorktreeReady, self.now()?)
            .map_err(domain_error)?;
        let transition = self.next_transition(&task, previous, "git.worktree.ready")?;
        isolation.status = GitIsolationStatus::WorktreeReady;
        isolation.expected_task_version = task.version();
        isolation.branch_created_by_app = true;
        isolation.worktree_created_by_app = true;
        isolation.updated_at_ms = task.updated_at_ms();
        if let Err(error) = self.repository.save_worktree_completion(
            expected_version,
            &task,
            &transition,
            &isolation,
        ) {
            return self.recover_after_completion_write_failure(
                task.id(),
                error,
                "git.worktree.persistence-failed",
            );
        }
        Ok(view(&task, &isolation, None))
    }

    fn mark_recovery(
        &mut self,
        mut task: Task,
        mut isolation: TaskGitIsolation,
        blocker: IsolationBlocker,
        reason_code: &str,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let expected_version = task.version();
        let previous = task.state();
        task.transition_to(TaskState::RecoveryRequired, self.now()?)
            .map_err(domain_error)?;
        let transition = self.next_transition(&task, previous, reason_code)?;
        isolation.status = GitIsolationStatus::RecoveryRequired;
        isolation.expected_task_version = task.version();
        isolation.updated_at_ms = task.updated_at_ms();
        self.repository
            .save_isolation_transition(expected_version, &task, &transition, &isolation)
            .map_err(repository_error)?;
        Ok(view(&task, &isolation, Some(blocker)))
    }

    fn recover_after_completion_write_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        reason_code: &str,
    ) -> Result<TaskIsolationView, ApplicationError> {
        let persisted_task = match self.load_task(task_id) {
            Ok(task) => task,
            Err(_) => return Err(repository_error(original)),
        };
        let persisted_isolation = match self.load_isolation(task_id) {
            Ok(isolation) => isolation,
            Err(_) => return Err(repository_error(original)),
        };
        self.mark_recovery(
            persisted_task,
            persisted_isolation,
            IsolationBlocker::RecoveryRequired,
            reason_code,
        )
        .map_err(|_| repository_error(original))
    }

    fn load_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<chatoms_ports::repository::ProjectRecord, ApplicationError> {
        self.repository
            .get_project(project_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))
    }

    fn load_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<ProjectFilesystemIdentityRecord, ApplicationError> {
        self.repository
            .get_project_identity(project_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::Conflict))
    }

    fn identity_matches_inspection(
        &mut self,
        expected: &ProjectFilesystemIdentityRecord,
        inspection: &chatoms_ports::git::ProjectInspection,
    ) -> Result<bool, ApplicationError> {
        let actual_root = self
            .filesystem
            .inspect_supported_directory(&inspection.canonical_root)
            .map_err(port_error)?;
        let root_matches = expected.root_volume_serial_hex == actual_root.volume_serial_hex
            && expected.root_file_id_hex == actual_root.file_id_hex;
        let common_matches = match (
            expected.repository_kind,
            inspection.git_common_dir.as_deref(),
        ) {
            (RepositoryKind::NonGit, None) => true,
            (RepositoryKind::Git, Some(path)) => {
                let actual = self
                    .filesystem
                    .inspect_supported_directory(path)
                    .map_err(port_error)?;
                expected.git_common_volume_serial_hex.as_deref()
                    == Some(actual.volume_serial_hex.as_str())
                    && expected.git_common_file_id_hex.as_deref()
                        == Some(actual.file_id_hex.as_str())
            }
            _ => false,
        };
        Ok(
            root_matches
                && common_matches
                && expected.repository_kind == inspection.repository_kind,
        )
    }

    fn load_task(&mut self, task_id: TaskId) -> Result<Task, ApplicationError> {
        self.repository
            .get_task(task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))
    }

    fn load_expected_task(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<Task, ApplicationError> {
        let task = self.load_task(task_id)?;
        if task.version() != expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        Ok(task)
    }

    fn load_isolation(&mut self, task_id: TaskId) -> Result<TaskGitIsolation, ApplicationError> {
        self.repository
            .get_task_isolation(task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))
    }

    fn next_transition(
        &mut self,
        task: &Task,
        previous: TaskState,
        reason_code: &str,
    ) -> Result<TaskStateTransition, ApplicationError> {
        let sequence = self
            .repository
            .next_transition_sequence(task.id())
            .map_err(repository_error)?;
        TaskStateTransition::new(TaskStateTransitionSnapshot {
            id: TaskStateTransitionId::new(),
            task_id: task.id(),
            sequence,
            from_state: Some(previous),
            to_state: task.state(),
            task_version: task.version(),
            actor_kind: actor("application")?,
            reason_code: reason(reason_code)?,
            occurred_at_ms: task.updated_at_ms(),
        })
        .map_err(domain_error)
    }

    fn now(&mut self) -> Result<i64, ApplicationError> {
        self.time.now_ms().map_err(port_error)
    }

    fn record_receipt(
        &mut self,
        operation_id: GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
    ) -> Result<(), ApplicationError> {
        let now = self.now()?;
        self.repository
            .append_git_operation_receipt(operation_id, kind, evidence, now)
            .map_err(repository_error)
    }
}

fn root_identity(
    project: &ProjectRecord,
    identity: &ProjectFilesystemIdentityRecord,
) -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: Path::new(&project.root_path).to_path_buf(),
        volume_serial_hex: identity.root_volume_serial_hex.clone(),
        file_id_hex: identity.root_file_id_hex.clone(),
    }
}

fn common_identity(
    identity: &ProjectFilesystemIdentityRecord,
    path: &Path,
) -> Result<DirectoryIdentity, ApplicationError> {
    Ok(DirectoryIdentity {
        canonical_path: path.to_path_buf(),
        volume_serial_hex: identity
            .git_common_volume_serial_hex
            .clone()
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?,
        file_id_hex: identity
            .git_common_file_id_hex
            .clone()
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?,
    })
}

fn transition_record(
    task: &Task,
    sequence: u64,
    previous: TaskState,
    reason_code: &str,
) -> Result<TaskStateTransition, ApplicationError> {
    TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id: task.id(),
        sequence,
        from_state: Some(previous),
        to_state: task.state(),
        task_version: task.version(),
        actor_kind: actor("application")?,
        reason_code: reason(reason_code)?,
        occurred_at_ms: task.updated_at_ms(),
    })
    .map_err(domain_error)
}

fn exact_success_receipts(receipts: &[GitOperationReceipt]) -> bool {
    receipts.len() == 3
        && receipts[0].sequence == 1
        && receipts[0].kind == GitOperationReceiptKind::CommandStarted
        && receipts[1].sequence == 2
        && receipts[1].kind == GitOperationReceiptKind::CommandSucceeded
        && receipts[2].sequence == 3
        && receipts[2].kind == GitOperationReceiptKind::PostVerified
}

fn readiness_blocker(status: &RepositoryStatus) -> Option<IsolationBlocker> {
    if !status.clean {
        Some(IsolationBlocker::DirtyRepository)
    } else if status.detached_head {
        Some(IsolationBlocker::DetachedHead)
    } else if status.head_commit.is_none() {
        Some(IsolationBlocker::UnbornRepository)
    } else if status.current_branch.is_none() {
        Some(IsolationBlocker::MissingCurrentBranch)
    } else {
        None
    }
}

fn view(
    task: &Task,
    isolation: &TaskGitIsolation,
    blocker: Option<IsolationBlocker>,
) -> TaskIsolationView {
    TaskIsolationView {
        task_id: task.id(),
        project_id: task.project_id(),
        task_state: task.state(),
        task_version: task.version(),
        isolation_status: isolation.status,
        branch_identity: task.task_branch_identity().as_str().to_owned(),
        base_branch: isolation.base_branch.clone(),
        base_commit: isolation.base_commit.clone(),
        blocker,
    }
}

fn actor(value: &str) -> Result<ActorKind, ApplicationError> {
    ActorKind::from_str(value).map_err(domain_error)
}

fn reason(value: &str) -> Result<ReasonCode, ApplicationError> {
    ReasonCode::from_str(value).map_err(domain_error)
}

fn port_error(error: PortFailure) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn repository_error(error: chatoms_ports::repository::RepositoryError) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn domain_error(error: chatoms_domain::DomainError) -> ApplicationError {
    ApplicationError::from_domain(&error)
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
