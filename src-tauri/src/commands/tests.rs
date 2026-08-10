use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{Receiver, Sender},
};
use std::thread;
use std::time::Duration;

use chatoms_application::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
    error::ApplicationError,
};
use chatoms_domain::{Task, TaskId, TaskStateTransition};
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, PlatformCapabilityStatus, TimeProvider,
    error::{FailureCategory, PortFailure},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
    repository::{
        ActiveLease, FoundationRepository, ProjectSummary, RepositoryError, RepositoryErrorCode,
    },
};

use super::{REGISTERED_HANDLERS, projects, system, tasks};
use crate::{
    dto::HealthStateDto,
    state::{
        AppRuntime, CapabilityHandle, ManagedRuntime, RepositoryHandle, RuntimePorts,
        RuntimeResources, TimeProviderHandle,
    },
};

#[derive(Default)]
struct CallCounts {
    projects: AtomicUsize,
    active: AtomicUsize,
    task: AtomicUsize,
}

struct RepositoryFake {
    calls: Arc<CallCounts>,
}

impl FoundationRepository for RepositoryFake {
    fn create_task(
        &mut self,
        _task: &Task,
        _initial_transition: &TaskStateTransition,
        _lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }
    fn get_task(&mut self, _task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.calls.task.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn save_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }
    fn save_recovery_target(
        &mut self,
        _expected_version: u64,
        _task: &Task,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }
    fn terminate_task(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }
    fn list_task_transitions(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        Ok(Vec::new())
    }
    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.calls.projects.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }
    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.calls.active.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

fn operation_failed() -> RepositoryError {
    RepositoryError::new(RepositoryErrorCode::OperationFailed)
}

struct TimeFake;

impl TimeProvider for TimeFake {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        Ok(1)
    }
}

struct CapabilityFake;

impl PlatformCapabilityPort for CapabilityFake {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        Ok(PlatformCapabilities {
            secure_storage: PlatformCapabilityStatus::Supported,
            native_permissions: PlatformCapabilityStatus::Supported,
        })
    }
}

struct GitCapabilityFake {
    available: Result<bool, PortFailure>,
}

impl GitService for GitCapabilityFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.available
    }
    fn inspect_project(
        &mut self,
        _input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn repository_status(
        &mut self,
        _root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn validate_non_git_source(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn validate_repository_source(
        &mut self,
        _root: &std::path::Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn initialize_repository(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn has_commit_author(&mut self, _root: &std::path::Path) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn create_initial_snapshot(&mut self, _root: &std::path::Path) -> Result<String, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn create_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn verify_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

struct BlockingGitCapabilityFake {
    started: Sender<()>,
    release: Receiver<()>,
}

impl GitService for BlockingGitCapabilityFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.started
            .send(())
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        self.release
            .recv()
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        Ok(true)
    }

    fn inspect_project(
        &mut self,
        _input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn repository_status(
        &mut self,
        _root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn validate_non_git_source(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn validate_repository_source(
        &mut self,
        _root: &std::path::Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn initialize_repository(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn has_commit_author(&mut self, _root: &std::path::Path) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn create_initial_snapshot(&mut self, _root: &std::path::Path) -> Result<String, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn create_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn verify_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

fn ready_runtime(calls: Arc<CallCounts>) -> ManagedRuntime {
    ready_runtime_with_git(calls, Ok(true))
}

fn ready_runtime_with_git(
    calls: Arc<CallCounts>,
    available: Result<bool, PortFailure>,
) -> ManagedRuntime {
    ManagedRuntime::ready(AppRuntime::new(
        BootstrapStatus {
            storage_status: StorageStatus::Ready,
            database_status: DatabaseStatus::Ready,
            logging_status: LoggingStatus::Ready,
            active_task_status: ActiveTaskStatus::None,
            application_version: APPLICATION_VERSION,
            ready: true,
        },
        RuntimePorts {
            repository: RepositoryHandle::new(RepositoryFake { calls }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(GitCapabilityFake { available }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
            worktree_paths: crate::state::WorktreePathHandle::new(
                chatoms_platform::ManagedWorktreePaths::windows_from_environment()
                    .expect("test worktree paths"),
            ),
        },
        RuntimeResources::default(),
    ))
}

fn ready_runtime_with_blocking_git(
    calls: Arc<CallCounts>,
    started: Sender<()>,
    release: Receiver<()>,
) -> ManagedRuntime {
    ManagedRuntime::ready(AppRuntime::new(
        BootstrapStatus {
            storage_status: StorageStatus::Ready,
            database_status: DatabaseStatus::Ready,
            logging_status: LoggingStatus::Ready,
            active_task_status: ActiveTaskStatus::None,
            application_version: APPLICATION_VERSION,
            ready: true,
        },
        RuntimePorts {
            repository: RepositoryHandle::new(RepositoryFake { calls }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(BlockingGitCapabilityFake {
                started,
                release,
            }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
            worktree_paths: crate::state::WorktreePathHandle::new(
                chatoms_platform::ManagedWorktreePaths::windows_from_environment()
                    .expect("test worktree paths"),
            ),
        },
        RuntimeResources::default(),
    ))
}

#[test]
fn system_status_exposes_only_the_verified_git_capability_result() {
    let supported = ready_runtime_with_git(Arc::new(CallCounts::default()), Ok(true));
    assert_eq!(
        system::handle_get_system_status(&supported)
            .expect("supported status")
            .capabilities
            .git_execution,
        crate::dto::CapabilityStatusDto::Supported
    );
    for unavailable in [
        Ok(false),
        Err(PortFailure::new(FailureCategory::Unsupported)),
    ] {
        let runtime = ready_runtime_with_git(Arc::new(CallCounts::default()), unavailable);
        assert_eq!(
            system::handle_get_system_status(&runtime)
                .expect("unavailable status")
                .capabilities
                .git_execution,
            crate::dto::CapabilityStatusDto::Unavailable
        );
        assert!(
            projects::handle_list_projects(&runtime)
                .expect("Git capability does not block project list")
                .is_empty()
        );
    }
}

fn unavailable_runtime() -> ManagedRuntime {
    let category = FailureCategory::StorageUnavailable;
    ManagedRuntime::unavailable(
        ApplicationError::from_failure(
            category,
            category.default_severity(),
            category.default_retry(),
        ),
        None,
    )
}

#[test]
fn ready_system_and_empty_foundation_commands_use_services_once() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls.clone());
    assert_eq!(
        system::handle_get_version(&runtime)
            .expect("version")
            .version,
        "0.1.0"
    );
    assert_eq!(
        system::handle_get_health(&runtime).expect("health").status,
        HealthStateDto::Healthy
    );
    assert!(
        projects::handle_list_projects(&runtime)
            .expect("projects")
            .is_empty()
    );
    assert!(
        tasks::handle_get_active_task(&runtime)
            .expect("active task")
            .is_none()
    );
    assert_eq!(calls.projects.load(Ordering::SeqCst), 1);
    assert_eq!(calls.active.load(Ordering::SeqCst), 1);
}

#[test]
fn read_only_project_list_remains_available_while_git_capability_probe_is_running() {
    let (started_send, started_receive) = std::sync::mpsc::channel();
    let (release_send, release_receive) = std::sync::mpsc::channel();
    let calls = Arc::new(CallCounts::default());
    let runtime = Arc::new(ready_runtime_with_blocking_git(
        calls.clone(),
        started_send,
        release_receive,
    ));
    let probing_runtime = runtime.clone();
    let probe = thread::spawn(move || system::handle_get_system_status(&probing_runtime));

    started_receive
        .recv()
        .expect("Git capability probe started");
    let (list_send, list_receive) = std::sync::mpsc::channel();
    let list_runtime = runtime.clone();
    let list = thread::spawn(move || list_send.send(projects::handle_list_projects(&list_runtime)));
    let list_result = list_receive.recv_timeout(Duration::from_secs(1));

    release_send.send(()).expect("release Git capability probe");
    assert_eq!(
        probe
            .join()
            .expect("capability probe thread")
            .expect("system status")
            .capabilities
            .git_execution,
        crate::dto::CapabilityStatusDto::Supported
    );
    list.join()
        .expect("project list thread")
        .expect("project list result sent");
    assert!(
        list_result
            .expect("project list must not wait for a Git capability probe")
            .expect("project list remains read-only available")
            .is_empty()
    );
    assert_eq!(calls.projects.load(Ordering::SeqCst), 1);
}

#[test]
fn task_not_found_and_unavailable_state_return_stable_safe_errors() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls.clone());
    let task_id = TaskId::new().to_string();
    let error = tasks::handle_get_task(&runtime, &task_id).expect_err("missing task");
    assert_eq!(error.code, "APP_NOT_FOUND");
    assert_eq!(calls.task.load(Ordering::SeqCst), 1);
    assert!(!error.to_string().contains("SELECT"));
    assert!(!error.to_string().contains("C:\\"));

    let runtime = unavailable_runtime();
    assert_eq!(
        system::handle_get_health(&runtime)
            .expect("safe health")
            .status,
        HealthStateDto::Unavailable
    );
    for error in [
        projects::handle_list_projects(&runtime).expect_err("projects unavailable"),
        tasks::handle_get_active_task(&runtime).expect_err("tasks unavailable"),
    ] {
        assert_eq!(error.code, "APP_STORAGE_UNAVAILABLE");
        assert_eq!(error.message, "Secure local storage is unavailable.");
    }
}

#[test]
fn handler_allowlist_contains_only_approved_purpose_specific_commands() {
    assert_eq!(REGISTERED_HANDLERS.len(), 16);
    assert_eq!(
        REGISTERED_HANDLERS,
        [
            "get_version",
            "get_health",
            "get_system_status",
            "get_bootstrap_status",
            "get_legacy_migration_diagnostic",
            "list_projects",
            "inspect_project_candidate",
            "register_project",
            "get_project_git_status",
            "create_isolation_task",
            "get_task_isolation",
            "approve_git_initialization",
            "create_task_worktree",
            "get_active_task",
            "get_task",
            "list_task_history",
        ]
    );
    for forbidden in [
        "create_task",
        "transition_task",
        "run_shell",
        "git",
        "updater",
        "installer",
        "credentials",
    ] {
        assert!(!REGISTERED_HANDLERS.contains(&forbidden));
    }
}
