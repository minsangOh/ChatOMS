use std::sync::{Arc, Mutex};

use chatoms_application::{
    bootstrap::{BootstrapService, BootstrapStatus, DatabaseStatus, StorageStatus},
    error::ApplicationError,
    git_isolation::GitIsolationService,
    tasks::TaskService,
};
use chatoms_infrastructure::bootstrap::{
    DatabaseBootstrapAdapter, LegacyProjectPreflightAdapter, LoggingBootstrapAdapter,
    SharedDatabase, SharedLoggingGuard,
};
#[cfg(any(not(test), windows))]
use chatoms_infrastructure::git::GitCliAdapter;
#[cfg(not(test))]
use chatoms_platform::ManagedWorktreePaths;
use chatoms_platform::bootstrap::{
    StaticPlatformCapabilityAdapter, StorageBootstrapAdapter, SystemTimeProvider,
};
use chatoms_ports::{
    DatabaseBootstrapPort, LoggingBootstrapPort, PlatformCapabilityPort, StorageBootstrapPort,
    TimeProvider, error::FailureCategory, repository::FoundationRepository,
};
#[cfg(test)]
use chatoms_ports::{
    error::PortFailure,
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome, WorktreePathProvider,
    },
};
#[cfg(not(test))]
use chatoms_ports::{
    error::PortFailure,
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
};

use crate::state::{
    AppRuntime, CapabilityHandle, FilesystemIdentityHandle, GitServiceHandle,
    ImplementationRunRegistry, ManagedRuntime, PlanningRunRegistry, ProviderCapabilityHandle,
    RepositoryHandle, ReviewRunRegistry, RuntimePorts, RuntimeResources, TestingRunRegistry,
    TimeProviderHandle, WorktreePathHandle,
};

pub fn compose_runtime<S, D, L, R, T, C>(
    mut storage: S,
    mut database: D,
    mut logging: L,
    mut repository: R,
    mut time: T,
    capabilities: C,
    resources: RuntimeResources,
) -> ManagedRuntime
where
    S: StorageBootstrapPort,
    D: DatabaseBootstrapPort,
    L: LoggingBootstrapPort,
    R: FoundationRepository + Send + 'static,
    T: TimeProvider + Send + 'static,
    C: PlatformCapabilityPort + Send + 'static,
{
    let bootstrap_result =
        BootstrapService::new(&mut storage, &mut database, &mut logging, &mut repository)
            .bootstrap();

    match bootstrap_result {
        Ok(status) if status.ready => {
            let mut git = runtime_git_adapter();
            #[cfg(all(windows, not(test)))]
            let mut worktree_paths = match ManagedWorktreePaths::windows_from_environment() {
                Ok(paths) => paths,
                Err(error) => {
                    return ManagedRuntime::unavailable(
                        ApplicationError::from_categorized(&error),
                        Some(status),
                    );
                }
            };
            #[cfg(all(target_os = "macos", not(test)))]
            let mut worktree_paths = ManagedWorktreePaths::new(
                chatoms_platform::path::MacOsPathResolver,
                chatoms_platform::permissions::MacOsPermissionManager,
            );
            #[cfg(all(not(any(windows, target_os = "macos")), not(test)))]
            let mut worktree_paths = UnsupportedWorktreePaths;
            #[cfg(test)]
            let mut worktree_paths = TestWorktreePaths;
            #[cfg(all(windows, not(test)))]
            let mut filesystem = chatoms_platform::filesystem::WindowsFilesystemIdentity;
            #[cfg(all(not(windows), not(test)))]
            let mut filesystem = UnsupportedFilesystemIdentity;
            #[cfg(test)]
            let mut filesystem = TestFilesystemIdentity;
            if let Err(error) = GitIsolationService::new(
                &mut repository,
                &mut git,
                &mut filesystem,
                &mut worktree_paths,
                &mut time,
            )
            .reconcile_startup()
            {
                return ManagedRuntime::unavailable(error, Some(status));
            }
            if let Err(error) =
                TaskService::new(&mut repository, &mut time).reconcile_startup_planning()
            {
                return ManagedRuntime::unavailable(error, Some(status));
            }
            if let Err(error) =
                TaskService::new(&mut repository, &mut time).reconcile_startup_implementation()
            {
                return ManagedRuntime::unavailable(error, Some(status));
            }
            if let Err(error) =
                TaskService::new(&mut repository, &mut time).reconcile_startup_testing()
            {
                return ManagedRuntime::unavailable(error, Some(status));
            }
            if let Err(error) =
                TaskService::new(&mut repository, &mut time).reconcile_startup_reviewing()
            {
                return ManagedRuntime::unavailable(error, Some(status));
            }
            let preflight_dir = prepare_preflight_directory();
            ManagedRuntime::ready(AppRuntime::new(
                status,
                RuntimePorts {
                    repository: RepositoryHandle::new(repository),
                    time: TimeProviderHandle::new(time),
                    capabilities: CapabilityHandle::new(capabilities),
                    git: GitServiceHandle::new(git),
                    filesystem: FilesystemIdentityHandle::new(filesystem),
                    worktree_paths: WorktreePathHandle::new(worktree_paths),
                    provider_capabilities: ProviderCapabilityHandle::new(),
                    preflight_dir,
                    planning_runs: PlanningRunRegistry::new(),
                    implementation_runs: ImplementationRunRegistry::new(),
                    testing_runs: TestingRunRegistry::new(),
                    review_runs: ReviewRunRegistry::new(),
                },
                resources,
            ))
        }
        Ok(status) => {
            let error = unavailable_status_error(&status);
            ManagedRuntime::unavailable(error, Some(status))
        }
        Err(error) => {
            let diagnostic = resources.database.migration_diagnostic();
            ManagedRuntime::unavailable_with_migration_diagnostic(error, None, diagnostic)
        }
    }
}

#[cfg(not(test))]
fn runtime_git_adapter() -> RuntimeGitService {
    match GitCliAdapter::from_environment() {
        Ok(git) => RuntimeGitService::Available(Box::new(git)),
        Err(_) => RuntimeGitService::Unavailable,
    }
}

#[cfg(test)]
fn runtime_git_adapter() -> TestGitService {
    TestGitService
}

#[cfg(all(windows, not(test)))]
fn prepare_preflight_directory() -> Option<crate::state::PreflightDirectory> {
    use chatoms_platform::preflight::TrustedPreflightWorkingDirectory;
    let resolver = match chatoms_platform::path::WindowsPathResolver::from_environment() {
        Ok(r) => r,
        Err(_) => return None,
    };
    let permissions = chatoms_platform::permissions::WindowsPermissionManager;
    TrustedPreflightWorkingDirectory::prepare(&resolver, &permissions).ok()
}

#[cfg(any(not(windows), test))]
fn prepare_preflight_directory() -> Option<crate::state::PreflightDirectory> {
    None
}

#[cfg(not(test))]
enum RuntimeGitService {
    Available(Box<GitCliAdapter>),
    Unavailable,
}

#[cfg(not(test))]
impl GitService for RuntimeGitService {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        Ok(matches!(self, Self::Available(_)))
    }

    fn inspect_project(
        &mut self,
        input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        match self {
            Self::Available(git) => git.inspect_project(input),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn repository_status(
        &mut self,
        root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        match self {
            Self::Available(git) => git.repository_status(root),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn validate_non_git_source(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        match self {
            Self::Available(git) => git.validate_non_git_source(root),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn validate_repository_source(
        &mut self,
        root: &std::path::Path,
        base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        match self {
            Self::Available(git) => git.validate_repository_source(root, base_commit),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn initialize_repository(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        match self {
            Self::Available(git) => git.initialize_repository(root),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn has_commit_author(&mut self, root: &std::path::Path) -> Result<bool, PortFailure> {
        match self {
            Self::Available(git) => git.has_commit_author(root),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn create_initial_snapshot(&mut self, root: &std::path::Path) -> Result<String, PortFailure> {
        match self {
            Self::Available(git) => git.create_initial_snapshot(root),
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn create_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
        safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        match self {
            Self::Available(git) => {
                git.create_task_worktree(root, branch, base_commit, worktree, safety)
            }
            Self::Unavailable => Err(unavailable_git()),
        }
    }

    fn verify_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        match self {
            Self::Available(git) => git.verify_task_worktree(root, branch, base_commit, worktree),
            Self::Unavailable => Err(unavailable_git()),
        }
    }
}

#[cfg(not(test))]
fn unavailable_git() -> PortFailure {
    PortFailure::new(FailureCategory::Unsupported)
}

#[cfg(test)]
struct TestGitService;

#[cfg(test)]
impl GitService for TestGitService {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn inspect_project(
        &mut self,
        _input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn repository_status(
        &mut self,
        _root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn validate_non_git_source(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn validate_repository_source(
        &mut self,
        _root: &std::path::Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn initialize_repository(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn has_commit_author(&mut self, _root: &std::path::Path) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn create_initial_snapshot(&mut self, _root: &std::path::Path) -> Result<String, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn create_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn verify_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }
}

#[cfg(test)]
struct TestFilesystemIdentity;

#[cfg(test)]
impl FilesystemIdentityPort for TestFilesystemIdentity {
    fn inspect_supported_directory(
        &mut self,
        _path: &std::path::Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn verify_local_tree(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }

    fn acquire_guard(
        &mut self,
        _path: &std::path::Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }
}

#[cfg(test)]
struct TestWorktreePaths;

#[cfg(test)]
impl WorktreePathProvider for TestWorktreePaths {
    fn prepare_worktree_path(
        &mut self,
        _project_id: chatoms_domain::ProjectId,
        _task_id: chatoms_domain::TaskId,
    ) -> Result<std::path::PathBuf, PortFailure> {
        Err(PortFailure::new(FailureCategory::Internal))
    }
}

#[must_use]
pub fn production_runtime() -> ManagedRuntime {
    let paths = Arc::new(Mutex::new(None));
    let database = SharedDatabase::default();
    let logging_guard = SharedLoggingGuard::default();
    let resources = RuntimeResources {
        paths: paths.clone(),
        database: database.clone(),
        logging_guard: logging_guard.clone(),
    };

    #[cfg(windows)]
    let storage = match StorageBootstrapAdapter::windows_from_environment(paths.clone()) {
        Ok(storage) => storage,
        Err(error) => {
            return ManagedRuntime::unavailable(ApplicationError::from_categorized(&error), None);
        }
    };

    #[cfg(target_os = "macos")]
    let storage = StorageBootstrapAdapter::new(
        chatoms_platform::path::MacOsPathResolver,
        chatoms_platform::permissions::MacOsPermissionManager,
        paths.clone(),
    );

    #[cfg(not(any(windows, target_os = "macos")))]
    let storage = UnsupportedStorageBootstrap;

    #[cfg(windows)]
    let database_adapter = {
        let adapter = DatabaseBootstrapAdapter::new(paths.clone(), database.clone());
        match GitCliAdapter::from_environment() {
            Ok(git) => adapter.with_legacy_preflight(LegacyProjectPreflightAdapter::new(
                git,
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            )),
            Err(_) => adapter,
        }
    };
    #[cfg(not(windows))]
    let database_adapter = DatabaseBootstrapAdapter::new(paths.clone(), database.clone());
    let logging_adapter = LoggingBootstrapAdapter::new(paths, logging_guard);
    let repository = database.repository();

    compose_runtime(
        storage,
        database_adapter,
        logging_adapter,
        repository,
        SystemTimeProvider,
        StaticPlatformCapabilityAdapter,
        resources,
    )
}

fn unavailable_status_error(status: &BootstrapStatus) -> ApplicationError {
    let category = match status.storage_status {
        StorageStatus::Insecure => FailureCategory::StorageInsecure,
        StorageStatus::Unavailable => FailureCategory::StorageUnavailable,
        StorageStatus::Unsupported => FailureCategory::Unsupported,
        StorageStatus::Ready => match status.database_status {
            DatabaseStatus::Incompatible | DatabaseStatus::MigrationRequired => {
                FailureCategory::MigrationFailure
            }
            DatabaseStatus::Unavailable | DatabaseStatus::NotChecked => {
                FailureCategory::StorageUnavailable
            }
            DatabaseStatus::Ready | DatabaseStatus::Upgraded => FailureCategory::Internal,
        },
    };
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}

#[cfg(not(any(windows, target_os = "macos")))]
struct UnsupportedStorageBootstrap;

#[cfg(not(any(windows, target_os = "macos")))]
struct UnsupportedWorktreePaths;

#[cfg(not(any(windows, target_os = "macos")))]
impl chatoms_ports::git::WorktreePathProvider for UnsupportedWorktreePaths {
    fn prepare_worktree_path(
        &mut self,
        _project_id: chatoms_domain::ProjectId,
        _task_id: chatoms_domain::TaskId,
    ) -> Result<std::path::PathBuf, chatoms_ports::error::PortFailure> {
        Err(chatoms_ports::error::PortFailure::new(
            FailureCategory::Unsupported,
        ))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl StorageBootstrapPort for UnsupportedStorageBootstrap {
    fn prepare_secure_storage(
        &mut self,
    ) -> Result<chatoms_ports::StorageBootstrapState, chatoms_ports::error::PortFailure> {
        Ok(chatoms_ports::StorageBootstrapState::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chatoms_domain::{Task, TaskId, TaskStateTransition};
    use chatoms_ports::{
        DatabaseBootstrapState, LoggingBootstrapState, PlatformCapabilities,
        PlatformCapabilityStatus, StorageBootstrapState,
        error::PortFailure,
        repository::{ActiveLease, ProjectSummary, RepositoryError},
    };

    use crate::state::RuntimeState;

    use super::*;

    #[derive(Clone)]
    struct Calls(Arc<Mutex<Vec<&'static str>>>);

    impl Calls {
        fn push(&self, value: &'static str) {
            self.0.lock().expect("calls").push(value);
        }
    }

    struct StorageFake {
        calls: Calls,
        state: StorageBootstrapState,
    }

    impl StorageBootstrapPort for StorageFake {
        fn prepare_secure_storage(&mut self) -> Result<StorageBootstrapState, PortFailure> {
            self.calls.push("storage");
            Ok(self.state)
        }
    }

    struct DatabaseFake {
        calls: Calls,
        state: DatabaseBootstrapState,
    }

    impl DatabaseBootstrapPort for DatabaseFake {
        fn bootstrap_database(&mut self) -> Result<DatabaseBootstrapState, PortFailure> {
            self.calls.push("database");
            Ok(self.state)
        }
    }

    struct LoggingFake {
        calls: Calls,
        fail: bool,
    }

    impl LoggingBootstrapPort for LoggingFake {
        fn bootstrap_logging(&mut self) -> Result<LoggingBootstrapState, PortFailure> {
            self.calls.push("logging");
            if self.fail {
                Err(PortFailure::new(FailureCategory::LoggingFailure))
            } else {
                Ok(LoggingBootstrapState::Ready)
            }
        }
    }

    struct RepositoryFake {
        calls: Calls,
    }

    impl FoundationRepository for RepositoryFake {
        fn create_task(
            &mut self,
            _task: &Task,
            _initial_transition: &TaskStateTransition,
            _lease_acquired_at_ms: i64,
        ) -> Result<(), RepositoryError> {
            unreachable!("mutation is not part of startup")
        }
        fn get_task(&mut self, _task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
            unreachable!("task lookup is not part of startup")
        }
        fn save_transition(
            &mut self,
            _expected_version: u64,
            _task: &Task,
            _transition: &TaskStateTransition,
        ) -> Result<(), RepositoryError> {
            unreachable!("mutation is not part of startup")
        }
        fn save_recovery_target(
            &mut self,
            _expected_version: u64,
            _task: &Task,
        ) -> Result<(), RepositoryError> {
            unreachable!("mutation is not part of startup")
        }
        fn terminate_task(
            &mut self,
            _expected_version: u64,
            _task: &Task,
            _transition: &TaskStateTransition,
        ) -> Result<(), RepositoryError> {
            unreachable!("mutation is not part of startup")
        }
        fn list_task_transitions(
            &mut self,
            _task_id: TaskId,
        ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
            unreachable!("history is not part of startup")
        }
        fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
            unreachable!("project lookup is not part of startup")
        }
        fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
            self.calls.push("lease");
            Ok(None)
        }
        fn list_incomplete_git_operations(
            &mut self,
        ) -> Result<Vec<chatoms_ports::repository::GitOperationAttempt>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone, Copy)]
    struct TimeFake;

    impl TimeProvider for TimeFake {
        fn now_ms(&mut self) -> Result<i64, PortFailure> {
            Ok(1)
        }
    }

    #[derive(Clone, Copy)]
    struct CapabilityFake;

    impl PlatformCapabilityPort for CapabilityFake {
        fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
            Ok(PlatformCapabilities {
                secure_storage: PlatformCapabilityStatus::Supported,
                native_permissions: PlatformCapabilityStatus::Supported,
            })
        }
    }

    fn compose(
        storage: StorageBootstrapState,
        database: DatabaseBootstrapState,
        logging_fail: bool,
    ) -> (ManagedRuntime, Vec<&'static str>) {
        let calls = Calls(Arc::new(Mutex::new(Vec::new())));
        let runtime = compose_runtime(
            StorageFake {
                calls: calls.clone(),
                state: storage,
            },
            DatabaseFake {
                calls: calls.clone(),
                state: database,
            },
            LoggingFake {
                calls: calls.clone(),
                fail: logging_fail,
            },
            RepositoryFake {
                calls: calls.clone(),
            },
            TimeFake,
            CapabilityFake,
            RuntimeResources::default(),
        );
        let recorded = calls.0.lock().expect("calls").clone();
        (runtime, recorded)
    }

    #[test]
    fn successful_and_logging_degraded_startup_are_ready_in_order() {
        let (runtime, calls) = compose(
            StorageBootstrapState::Ready,
            DatabaseBootstrapState::Ready,
            false,
        );
        assert!(matches!(
            *runtime.lock().expect("state"),
            RuntimeState::Ready(_)
        ));
        assert_eq!(
            calls,
            [
                "storage", "database", "logging", "lease", "lease", "lease", "lease", "lease",
                "lease"
            ]
        );

        let (runtime, calls) = compose(
            StorageBootstrapState::Ready,
            DatabaseBootstrapState::Ready,
            true,
        );
        let state = runtime.lock().expect("state");
        let RuntimeState::Ready(ready) = &*state else {
            panic!("logging failure is degraded ready");
        };
        assert_eq!(
            ready.bootstrap_status.logging_status,
            chatoms_application::bootstrap::LoggingStatus::Unavailable
        );
        assert_eq!(
            calls,
            [
                "storage", "database", "logging", "lease", "lease", "lease", "lease", "lease",
                "lease"
            ]
        );
    }

    #[test]
    fn critical_statuses_are_unavailable_and_stop_followup_calls() {
        for (storage, database, expected_calls) in [
            (
                StorageBootstrapState::Insecure,
                DatabaseBootstrapState::Ready,
                vec!["storage"],
            ),
            (
                StorageBootstrapState::Ready,
                DatabaseBootstrapState::Unavailable,
                vec!["storage", "database"],
            ),
            (
                StorageBootstrapState::Ready,
                DatabaseBootstrapState::Incompatible,
                vec!["storage", "database"],
            ),
        ] {
            let (runtime, calls) = compose(storage, database, false);
            let state = runtime.lock().expect("state");
            let RuntimeState::Unavailable(unavailable) = &*state else {
                panic!("critical status must be unavailable");
            };
            assert!(unavailable.error.to_string().starts_with("APP_"));
            assert_eq!(calls, expected_calls);
        }
    }
}
