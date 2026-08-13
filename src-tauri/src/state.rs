use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, MutexGuard, RwLock,
    atomic::{AtomicU64, Ordering},
};

use chatoms_application::system::CapabilityStatus as AppCapabilityStatus;
use chatoms_application::{bootstrap::BootstrapStatus, error::ApplicationError};
use chatoms_domain::{ProjectId, Task, TaskId, TaskStateTransition, WorkKind};
use chatoms_infrastructure::bootstrap::{
    LegacyMigrationDiagnostic, SharedDatabase, SharedFoundationRepository, SharedLoggingGuard,
    SharedResolvedAppPaths,
};
use chatoms_ports::process::AtomicCancellationSignal;
#[cfg(windows)]
pub type PreflightDirectory = chatoms_platform::preflight::TrustedPreflightWorkingDirectory;

#[cfg(not(windows))]
#[derive(Clone, Debug)]
pub struct PreflightDirectory;

#[cfg(not(windows))]
impl PreflightDirectory {
    pub fn revalidate(&self) -> Result<(), chatoms_ports::error::PortFailure> {
        Err(chatoms_ports::error::PortFailure::new(
            chatoms_ports::error::FailureCategory::Unsupported,
        ))
    }

    pub fn path(&self) -> &std::path::Path {
        std::path::Path::new(".")
    }
}
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, TimeProvider,
    error::PortFailure,
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome, WorktreePathProvider,
    },
    provider::ProviderKind,
    repository::{
        ActiveLease, AppProfileRecord, FoundationRepository, GitInitApproval, GitOperationAttempt,
        GitOperationReceipt, GitOperationReceiptKind, ProjectFilesystemIdentityRecord,
        ProjectRecord, ProjectSummary, ProviderBindingRecord, ProviderConsent, RepositoryError,
        TaskBriefRecord, TaskGitIsolation, TaskPlanningResultRecord,
    },
};

use crate::error::IpcErrorDto;

#[derive(Clone)]
pub struct RepositoryHandle {
    inner: Arc<Mutex<Box<dyn FoundationRepository + Send>>>,
}

impl RepositoryHandle {
    pub fn new(repository: impl FoundationRepository + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(repository))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn FoundationRepository) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RepositoryError::new(chatoms_ports::repository::RepositoryErrorCode::OperationFailed)
        })?;
        operation(inner.as_mut())
    }
}

impl FoundationRepository for RepositoryHandle {
    fn create_project(&mut self, project: &ProjectRecord) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.create_project(project))
    }

    fn create_project_with_identity(
        &mut self,
        project: &ProjectRecord,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.create_project_with_identity(project, identity))
    }

    fn get_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_project_identity(project_id))
    }

    fn update_project_identity(
        &mut self,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.update_project_identity(identity))
    }

    fn get_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_project(project_id))
    }

    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.create_task(task, initial_transition, lease_acquired_at_ms))
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.with_inner(|inner| inner.get_task(task_id))
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_transition(expected_version, task, transition))
    }

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_recovery_target(expected_version, task))
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.terminate_task(expected_version, task, transition))
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        self.with_inner(|inner| inner.list_task_transitions(task_id))
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.with_inner(|inner| inner.list_projects())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.with_inner(|inner| inner.active_lease())
    }

    fn create_isolation_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        classified_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
        isolation: &TaskGitIsolation,
        brief: Option<&TaskBriefRecord>,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.create_isolation_task(
                task,
                initial_transition,
                classified_transition,
                lease_acquired_at_ms,
                isolation,
                brief,
            )
        })
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_isolation(task_id))
    }

    fn get_task_brief(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskBriefRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_brief(task_id))
    }

    fn get_task_planning_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<chatoms_ports::repository::TaskPlanningResultRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_planning_result(task_id))
    }

    fn get_provider_consent(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_provider_consent(task_id, provider, work_kind, approved_task_version)
        })
    }

    fn save_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_planning_transition(expected_version, task, transition, consent)
        })
    }

    fn save_planning_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskPlanningResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_planning_result(expected_version, task, transition, result, terminal)
        })
    }

    fn begin_git_initialization(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
        approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.begin_git_initialization(expected_version, isolation, approval)
        })
    }

    fn save_isolation_intent(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_isolation_intent(expected_version, isolation))
    }

    fn append_git_operation_receipt(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.append_git_operation_receipt(operation_id, kind, evidence, recorded_at_ms)
        })
    }

    fn list_git_operation_receipts(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        self.with_inner(|inner| inner.list_git_operation_receipts(operation_id))
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        self.with_inner(|inner| inner.list_incomplete_git_operations())
    }

    fn save_isolation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_isolation_transition(expected_version, task, transition, isolation)
        })
    }

    fn save_git_initialization_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_git_initialization_completion(
                expected_version,
                task,
                transition,
                isolation,
                identity,
            )
        })
    }

    fn save_worktree_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_worktree_completion(expected_version, task, transition, isolation)
        })
    }

    fn terminate_isolation_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.terminate_isolation_task(expected_version, task, transition, isolation)
        })
    }

    fn ensure_default_profile_and_claude_binding(
        &mut self,
        profile: &AppProfileRecord,
        binding: &ProviderBindingRecord,
    ) -> Result<ProviderBindingRecord, RepositoryError> {
        self.with_inner(|inner| inner.ensure_default_profile_and_claude_binding(profile, binding))
    }

    fn get_claude_binding(
        &mut self,
        profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_claude_binding(profile_name))
    }

    fn update_claude_executable_path(
        &mut self,
        binding_id: &str,
        executable_path: Option<&str>,
        updated_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.update_claude_executable_path(binding_id, executable_path, updated_at_ms)
        })
    }
}

#[derive(Clone)]
pub struct GitServiceHandle {
    inner: Arc<Mutex<Box<dyn GitService + Send>>>,
}

impl GitServiceHandle {
    pub fn new(service: impl GitService + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(service))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn GitService) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl GitService for GitServiceHandle {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.with_inner(|inner| inner.is_available())
    }
    fn inspect_project(
        &mut self,
        input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        self.with_inner(|inner| inner.inspect_project(input))
    }
    fn repository_status(
        &mut self,
        root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        self.with_inner(|inner| inner.repository_status(root))
    }
    fn validate_non_git_source(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.with_inner(|inner| inner.validate_non_git_source(root))
    }
    fn validate_repository_source(
        &mut self,
        root: &std::path::Path,
        base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        self.with_inner(|inner| inner.validate_repository_source(root, base_commit))
    }
    fn initialize_repository(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.with_inner(|inner| inner.initialize_repository(root))
    }
    fn has_commit_author(&mut self, root: &std::path::Path) -> Result<bool, PortFailure> {
        self.with_inner(|inner| inner.has_commit_author(root))
    }
    fn create_initial_snapshot(&mut self, root: &std::path::Path) -> Result<String, PortFailure> {
        self.with_inner(|inner| inner.create_initial_snapshot(root))
    }
    fn create_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
        safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        self.with_inner(|inner| {
            inner.create_task_worktree(root, branch, base_commit, worktree, safety)
        })
    }
    fn verify_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        self.with_inner(|inner| inner.verify_task_worktree(root, branch, base_commit, worktree))
    }
}

#[derive(Clone)]
pub struct WorktreePathHandle {
    inner: Arc<Mutex<Box<dyn WorktreePathProvider + Send>>>,
}

#[derive(Clone)]
pub struct FilesystemIdentityHandle {
    inner: Arc<Mutex<Box<dyn FilesystemIdentityPort + Send>>>,
}

impl FilesystemIdentityHandle {
    pub fn new(port: impl FilesystemIdentityPort + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(port))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn FilesystemIdentityPort) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl FilesystemIdentityPort for FilesystemIdentityHandle {
    fn inspect_supported_directory(
        &mut self,
        path: &std::path::Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.with_inner(|inner| inner.inspect_supported_directory(path))
    }

    fn verify_local_tree(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.with_inner(|inner| inner.verify_local_tree(root))
    }

    fn acquire_guard(
        &mut self,
        path: &std::path::Path,
        expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        self.with_inner(|inner| inner.acquire_guard(path, expected))
    }
}

impl WorktreePathHandle {
    pub fn new(provider: impl WorktreePathProvider + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(provider))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn WorktreePathProvider) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl WorktreePathProvider for WorktreePathHandle {
    fn prepare_worktree_path(
        &mut self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<std::path::PathBuf, PortFailure> {
        self.with_inner(|inner| inner.prepare_worktree_path(project_id, task_id))
    }
}

#[derive(Clone)]
pub struct TimeProviderHandle {
    inner: Arc<Mutex<Box<dyn TimeProvider + Send>>>,
}

impl TimeProviderHandle {
    pub fn new(provider: impl TimeProvider + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(provider))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn TimeProvider) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl TimeProvider for TimeProviderHandle {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        self.with_inner(|inner| inner.now_ms())
    }
}

#[derive(Clone)]
pub struct CapabilityHandle {
    inner: Arc<Mutex<Box<dyn PlatformCapabilityPort + Send>>>,
}

impl CapabilityHandle {
    pub fn new(adapter: impl PlatformCapabilityPort + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(adapter))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn PlatformCapabilityPort) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl PlatformCapabilityPort for CapabilityHandle {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        self.with_inner(|inner| inner.platform_capabilities())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CachedProviderCapabilities {
    pub claude: Option<AppCapabilityStatus>,
    pub codex: Option<AppCapabilityStatus>,
}

impl CachedProviderCapabilities {
    const NOT_YET_PROBED: Self = Self {
        claude: None,
        codex: None,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    Completed,
    Superseded,
    Conflict,
}

#[derive(Clone)]
pub struct ProviderCapabilityHandle {
    generation: Arc<AtomicU64>,
    cache: Arc<RwLock<CachedProviderCapabilities>>,
    refreshing: Arc<Mutex<bool>>,
}

impl Default for ProviderCapabilityHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderCapabilityHandle {
    #[must_use]
    pub fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            cache: Arc::new(RwLock::new(CachedProviderCapabilities::NOT_YET_PROBED)),
            refreshing: Arc::new(Mutex::new(false)),
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn read_cache(&self) -> CachedProviderCapabilities {
        self.cache
            .read()
            .map(|guard| *guard)
            .unwrap_or(CachedProviderCapabilities::NOT_YET_PROBED)
    }

    pub fn invalidate_and_bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cache) = self.cache.write() {
            *cache = CachedProviderCapabilities::NOT_YET_PROBED;
        }
    }

    pub fn try_begin_refresh(&self) -> Option<u64> {
        let mut refreshing = self.refreshing.lock().ok()?;
        if *refreshing {
            return None;
        }
        *refreshing = true;
        Some(self.generation.load(Ordering::Acquire))
    }

    pub fn finish_refresh(
        &self,
        captured_generation: u64,
        capabilities: CachedProviderCapabilities,
    ) -> RefreshOutcome {
        let result = {
            let current_generation = self.generation.load(Ordering::Acquire);
            if current_generation != captured_generation {
                RefreshOutcome::Superseded
            } else if let Ok(mut cache) = self.cache.write() {
                *cache = capabilities;
                RefreshOutcome::Completed
            } else {
                RefreshOutcome::Superseded
            }
        };
        if let Ok(mut refreshing) = self.refreshing.lock() {
            *refreshing = false;
        }
        result
    }

    pub fn abort_refresh(&self) {
        if let Ok(mut refreshing) = self.refreshing.lock() {
            *refreshing = false;
        }
    }
}

/// In-memory-only registry of cancellation handles for Claude Planning runs
/// currently executing on a background thread, keyed by task id. Never
/// persisted: an app restart has no running process to cancel anyway, and
/// `docs/PRODUCT_REQUIREMENTS.md`/AGENTS.md scope this Unit to an in-memory
/// handle, not a separate execution/session table.
#[derive(Clone, Default)]
pub struct PlanningRunRegistry {
    inner: Arc<Mutex<HashMap<TaskId, AtomicCancellationSignal>>>,
}

impl PlanningRunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh cancellation signal for `task_id`. Returns `None`
    /// instead of overwriting an existing entry: two live handles for the
    /// same task would mean two independent things could race to cancel (or
    /// fail to unregister) the same run. `TaskService::start_planning`
    /// already fails closed (via optimistic task-version concurrency) before
    /// a second Planning attempt for the same task could ever reach this
    /// call, so a `None` here indicates a caller invariant violation, not
    /// routine contention, and the caller must not silently proceed as if a
    /// handle had been registered.
    pub fn register(&self, task_id: TaskId) -> Option<AtomicCancellationSignal> {
        let signal = AtomicCancellationSignal::new();
        let mut guard = self.inner.lock().ok()?;
        if guard.contains_key(&task_id) {
            return None;
        }
        guard.insert(task_id, signal.clone());
        Some(signal)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }

    /// Requests cancellation of the in-flight run for `task_id`, if any.
    /// Returns whether a matching run was found; the caller must not infer
    /// anything about the eventual outcome from this alone — only a
    /// subsequently *confirmed* process exit is recorded as `Cancelled`.
    pub fn request_cancellation(&self, task_id: TaskId) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        let Some(signal) = guard.get(&task_id) else {
            return false;
        };
        signal.cancel();
        true
    }
}

#[derive(Clone, Default)]
pub struct RuntimeResources {
    pub paths: SharedResolvedAppPaths,
    pub database: SharedDatabase,
    pub logging_guard: SharedLoggingGuard,
}

#[derive(Clone)]
pub struct AppRuntime {
    pub bootstrap_status: BootstrapStatus,
    pub repository: RepositoryHandle,
    pub time: TimeProviderHandle,
    pub capabilities: CapabilityHandle,
    pub git: GitServiceHandle,
    pub filesystem: FilesystemIdentityHandle,
    pub worktree_paths: WorktreePathHandle,
    pub provider_capabilities: ProviderCapabilityHandle,
    pub preflight_dir: Option<PreflightDirectory>,
    pub planning_runs: PlanningRunRegistry,
    resources: RuntimeResources,
}

pub struct RuntimePorts {
    pub repository: RepositoryHandle,
    pub time: TimeProviderHandle,
    pub capabilities: CapabilityHandle,
    pub git: GitServiceHandle,
    pub filesystem: FilesystemIdentityHandle,
    pub worktree_paths: WorktreePathHandle,
    pub provider_capabilities: ProviderCapabilityHandle,
    pub preflight_dir: Option<PreflightDirectory>,
    pub planning_runs: PlanningRunRegistry,
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
            provider_capabilities: ports.provider_capabilities,
            preflight_dir: ports.preflight_dir,
            planning_runs: ports.planning_runs,
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

pub enum RuntimeSnapshot {
    Ready(AppRuntime),
    Unavailable {
        error: ApplicationError,
        bootstrap_status: Option<BootstrapStatus>,
        migration_diagnostic: Option<LegacyMigrationDiagnostic>,
    },
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

    pub fn snapshot(&self) -> Result<RuntimeSnapshot, IpcErrorDto> {
        let state = self.lock()?;
        Ok(match &*state {
            RuntimeState::Ready(ready) => RuntimeSnapshot::Ready(ready.clone()),
            RuntimeState::Unavailable(unavailable) => RuntimeSnapshot::Unavailable {
                error: unavailable.error.clone(),
                bootstrap_status: unavailable.bootstrap_status.clone(),
                migration_diagnostic: unavailable.migration_diagnostic.clone(),
            },
        })
    }

    pub fn ready_snapshot(&self) -> Result<AppRuntime, IpcErrorDto> {
        match self.snapshot()? {
            RuntimeSnapshot::Ready(ready) => Ok(ready),
            RuntimeSnapshot::Unavailable { error, .. } => Err(error.into()),
        }
    }
}

impl From<SharedFoundationRepository> for RepositoryHandle {
    fn from(repository: SharedFoundationRepository) -> Self {
        Self::new(repository)
    }
}

#[cfg(test)]
mod planning_run_registry_tests {
    use super::PlanningRunRegistry;
    use chatoms_domain::TaskId;
    use chatoms_ports::process::CancellationSignal;

    #[test]
    fn cancellation_reaches_the_registered_signal_and_reports_it_was_found() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(!signal.is_cancelled());
        assert!(registry.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn cancellation_for_an_unregistered_task_reports_nothing_found() {
        let registry = PlanningRunRegistry::new();
        assert!(!registry.request_cancellation(TaskId::new()));
    }

    #[test]
    fn unregistering_removes_the_entry_so_a_later_cancel_reports_nothing_found() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let _signal = registry.register(task_id).expect("first registration");

        registry.unregister(task_id);

        assert!(!registry.request_cancellation(task_id));
    }

    #[test]
    fn a_clone_shares_the_same_underlying_registry() {
        let registry = PlanningRunRegistry::new();
        let clone = registry.clone();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(clone.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn registering_again_for_the_same_task_id_is_rejected_and_the_prior_signal_still_works() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");

        assert!(
            registry.register(task_id).is_none(),
            "a second registration for a task id already registered must be rejected"
        );

        assert!(registry.request_cancellation(task_id));
        assert!(
            first.is_cancelled(),
            "the original signal must still be the one in the registry"
        );
    }

    #[test]
    fn registering_again_after_unregistering_succeeds() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");
        registry.unregister(task_id);

        let second = registry
            .register(task_id)
            .expect("registration after unregister must succeed");

        assert!(registry.request_cancellation(task_id));
        assert!(
            !first.is_cancelled(),
            "the stale signal must not be reachable anymore"
        );
        assert!(second.is_cancelled());
    }
}
