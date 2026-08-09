use std::sync::{Mutex, MutexGuard};

use chatoms_application::{bootstrap::BootstrapStatus, error::ApplicationError};
use chatoms_domain::{ProjectId, Task, TaskId, TaskStateTransition};
use chatoms_infrastructure::bootstrap::{
    LegacyMigrationDiagnostic, SharedDatabase, SharedFoundationRepository, SharedLoggingGuard,
    SharedResolvedAppPaths,
};
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, TimeProvider,
    error::PortFailure,
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome, WorktreePathProvider,
    },
    repository::{
        ActiveLease, FoundationRepository, GitInitApproval, GitOperationAttempt,
        GitOperationReceipt, GitOperationReceiptKind, ProjectFilesystemIdentityRecord,
        ProjectRecord, ProjectSummary, RepositoryError, TaskGitIsolation,
    },
};

use crate::error::IpcErrorDto;

pub struct RepositoryHandle {
    inner: Box<dyn FoundationRepository + Send>,
}

impl RepositoryHandle {
    pub fn new(repository: impl FoundationRepository + Send + 'static) -> Self {
        Self {
            inner: Box::new(repository),
        }
    }
}

impl FoundationRepository for RepositoryHandle {
    fn create_project(&mut self, project: &ProjectRecord) -> Result<(), RepositoryError> {
        self.inner.create_project(project)
    }

    fn create_project_with_identity(
        &mut self,
        project: &ProjectRecord,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.inner.create_project_with_identity(project, identity)
    }

    fn get_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        self.inner.get_project_identity(project_id)
    }

    fn update_project_identity(
        &mut self,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.inner.update_project_identity(identity)
    }

    fn get_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        self.inner.get_project(project_id)
    }

    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.inner
            .create_task(task, initial_transition, lease_acquired_at_ms)
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.inner.get_task(task_id)
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.inner
            .save_transition(expected_version, task, transition)
    }

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        self.inner.save_recovery_target(expected_version, task)
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.inner
            .terminate_task(expected_version, task, transition)
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        self.inner.list_task_transitions(task_id)
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.inner.list_projects()
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.inner.active_lease()
    }

    fn create_isolation_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        classified_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.inner.create_isolation_task(
            task,
            initial_transition,
            classified_transition,
            lease_acquired_at_ms,
            isolation,
        )
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        self.inner.get_task_isolation(task_id)
    }

    fn begin_git_initialization(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
        approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        self.inner
            .begin_git_initialization(expected_version, isolation, approval)
    }

    fn save_isolation_intent(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.inner
            .save_isolation_intent(expected_version, isolation)
    }

    fn append_git_operation_receipt(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.inner
            .append_git_operation_receipt(operation_id, kind, evidence, recorded_at_ms)
    }

    fn list_git_operation_receipts(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        self.inner.list_git_operation_receipts(operation_id)
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        self.inner.list_incomplete_git_operations()
    }

    fn save_isolation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.inner
            .save_isolation_transition(expected_version, task, transition, isolation)
    }

    fn save_git_initialization_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.inner.save_git_initialization_completion(
            expected_version,
            task,
            transition,
            isolation,
            identity,
        )
    }

    fn save_worktree_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.inner
            .save_worktree_completion(expected_version, task, transition, isolation)
    }

    fn terminate_isolation_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.inner
            .terminate_isolation_task(expected_version, task, transition, isolation)
    }
}

pub struct GitServiceHandle {
    inner: Box<dyn GitService + Send>,
}

impl GitServiceHandle {
    pub fn new(service: impl GitService + Send + 'static) -> Self {
        Self {
            inner: Box::new(service),
        }
    }
}

impl GitService for GitServiceHandle {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.inner.is_available()
    }
    fn inspect_project(
        &mut self,
        input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        self.inner.inspect_project(input)
    }
    fn repository_status(
        &mut self,
        root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        self.inner.repository_status(root)
    }
    fn validate_non_git_source(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.inner.validate_non_git_source(root)
    }
    fn validate_repository_source(
        &mut self,
        root: &std::path::Path,
        base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        self.inner.validate_repository_source(root, base_commit)
    }
    fn initialize_repository(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.inner.initialize_repository(root)
    }
    fn has_commit_author(&mut self, root: &std::path::Path) -> Result<bool, PortFailure> {
        self.inner.has_commit_author(root)
    }
    fn create_initial_snapshot(&mut self, root: &std::path::Path) -> Result<String, PortFailure> {
        self.inner.create_initial_snapshot(root)
    }
    fn create_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
        safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        self.inner
            .create_task_worktree(root, branch, base_commit, worktree, safety)
    }
    fn verify_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        self.inner
            .verify_task_worktree(root, branch, base_commit, worktree)
    }
}

pub struct WorktreePathHandle {
    inner: Box<dyn WorktreePathProvider + Send>,
}

pub struct FilesystemIdentityHandle {
    inner: Box<dyn FilesystemIdentityPort + Send>,
}

impl FilesystemIdentityHandle {
    pub fn new(port: impl FilesystemIdentityPort + Send + 'static) -> Self {
        Self {
            inner: Box::new(port),
        }
    }
}

impl FilesystemIdentityPort for FilesystemIdentityHandle {
    fn inspect_supported_directory(
        &mut self,
        path: &std::path::Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.inner.inspect_supported_directory(path)
    }

    fn verify_local_tree(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.inner.verify_local_tree(root)
    }

    fn acquire_guard(
        &mut self,
        path: &std::path::Path,
        expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        self.inner.acquire_guard(path, expected)
    }
}

impl WorktreePathHandle {
    pub fn new(provider: impl WorktreePathProvider + Send + 'static) -> Self {
        Self {
            inner: Box::new(provider),
        }
    }
}

impl WorktreePathProvider for WorktreePathHandle {
    fn prepare_worktree_path(
        &mut self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<std::path::PathBuf, PortFailure> {
        self.inner.prepare_worktree_path(project_id, task_id)
    }
}

pub struct TimeProviderHandle {
    inner: Box<dyn TimeProvider + Send>,
}

impl TimeProviderHandle {
    pub fn new(provider: impl TimeProvider + Send + 'static) -> Self {
        Self {
            inner: Box::new(provider),
        }
    }
}

impl TimeProvider for TimeProviderHandle {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        self.inner.now_ms()
    }
}

pub struct CapabilityHandle {
    inner: Box<dyn PlatformCapabilityPort + Send>,
}

impl CapabilityHandle {
    pub fn new(adapter: impl PlatformCapabilityPort + Send + 'static) -> Self {
        Self {
            inner: Box::new(adapter),
        }
    }
}

impl PlatformCapabilityPort for CapabilityHandle {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        self.inner.platform_capabilities()
    }
}

#[derive(Clone, Default)]
pub struct RuntimeResources {
    pub paths: SharedResolvedAppPaths,
    pub database: SharedDatabase,
    pub logging_guard: SharedLoggingGuard,
}

pub struct AppRuntime {
    pub bootstrap_status: BootstrapStatus,
    pub repository: RepositoryHandle,
    pub time: TimeProviderHandle,
    pub capabilities: CapabilityHandle,
    pub git: GitServiceHandle,
    pub filesystem: FilesystemIdentityHandle,
    pub worktree_paths: WorktreePathHandle,
    resources: RuntimeResources,
}

pub struct RuntimePorts {
    pub repository: RepositoryHandle,
    pub time: TimeProviderHandle,
    pub capabilities: CapabilityHandle,
    pub git: GitServiceHandle,
    pub filesystem: FilesystemIdentityHandle,
    pub worktree_paths: WorktreePathHandle,
}

impl AppRuntime {
    pub fn new(
        bootstrap_status: BootstrapStatus,
        ports: RuntimePorts,
        resources: RuntimeResources,
    ) -> Self {
        Self {
            bootstrap_status,
            repository: ports.repository,
            time: ports.time,
            capabilities: ports.capabilities,
            git: ports.git,
            filesystem: ports.filesystem,
            worktree_paths: ports.worktree_paths,
            resources,
        }
    }

    #[must_use]
    pub fn logging_guard_is_initialized(&self) -> bool {
        self.resources.logging_guard.is_initialized()
    }

    #[must_use]
    pub fn database_is_initialized(&self) -> bool {
        self.resources.database.is_initialized()
    }

    #[must_use]
    pub fn has_resolved_paths(&self) -> bool {
        self.resources
            .paths
            .lock()
            .map(|paths| paths.is_some())
            .unwrap_or(false)
    }
}

pub struct UnavailableRuntime {
    pub error: ApplicationError,
    pub bootstrap_status: Option<BootstrapStatus>,
    pub migration_diagnostic: Option<LegacyMigrationDiagnostic>,
}

pub enum RuntimeState {
    Ready(AppRuntime),
    Unavailable(UnavailableRuntime),
}

pub struct ManagedRuntime {
    inner: Mutex<RuntimeState>,
}

impl ManagedRuntime {
    #[must_use]
    pub fn ready(runtime: AppRuntime) -> Self {
        Self {
            inner: Mutex::new(RuntimeState::Ready(runtime)),
        }
    }

    #[must_use]
    pub fn unavailable(error: ApplicationError, bootstrap_status: Option<BootstrapStatus>) -> Self {
        Self {
            inner: Mutex::new(RuntimeState::Unavailable(UnavailableRuntime {
                error,
                bootstrap_status,
                migration_diagnostic: None,
            })),
        }
    }

    #[must_use]
    pub fn unavailable_with_migration_diagnostic(
        error: ApplicationError,
        bootstrap_status: Option<BootstrapStatus>,
        migration_diagnostic: Option<LegacyMigrationDiagnostic>,
    ) -> Self {
        Self {
            inner: Mutex::new(RuntimeState::Unavailable(UnavailableRuntime {
                error,
                bootstrap_status,
                migration_diagnostic,
            })),
        }
    }

    pub fn lock(&self) -> Result<MutexGuard<'_, RuntimeState>, IpcErrorDto> {
        self.inner.lock().map_err(|_| IpcErrorDto::internal())
    }
}

impl From<SharedFoundationRepository> for RepositoryHandle {
    fn from(repository: SharedFoundationRepository) -> Self {
        Self::new(repository)
    }
}
