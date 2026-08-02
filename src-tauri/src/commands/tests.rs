use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use chatoms_application::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
    error::ApplicationError,
};
use chatoms_domain::{Task, TaskId, TaskStateTransition};
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, PlatformCapabilityStatus, TimeProvider,
    error::{FailureCategory, PortFailure},
    repository::{
        ActiveLease, FoundationRepository, ProjectSummary, RepositoryError, RepositoryErrorCode,
    },
};

use super::{REGISTERED_HANDLERS, projects, system, tasks};
use crate::{
    dto::HealthStateDto,
    state::{
        AppRuntime, CapabilityHandle, ManagedRuntime, RepositoryHandle, RuntimeResources,
        TimeProviderHandle,
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

fn ready_runtime(calls: Arc<CallCounts>) -> ManagedRuntime {
    ManagedRuntime::ready(AppRuntime::new(
        BootstrapStatus {
            storage_status: StorageStatus::Ready,
            database_status: DatabaseStatus::Ready,
            logging_status: LoggingStatus::Ready,
            active_task_status: ActiveTaskStatus::None,
            application_version: APPLICATION_VERSION,
            ready: true,
        },
        RepositoryHandle::new(RepositoryFake { calls }),
        TimeProviderHandle::new(TimeFake),
        CapabilityHandle::new(CapabilityFake),
        RuntimeResources::default(),
    ))
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
fn handler_allowlist_contains_only_approved_read_only_commands() {
    assert_eq!(REGISTERED_HANDLERS.len(), 8);
    assert_eq!(
        REGISTERED_HANDLERS,
        [
            "get_version",
            "get_health",
            "get_system_status",
            "get_bootstrap_status",
            "list_projects",
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
