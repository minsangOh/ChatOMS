use std::sync::{Mutex, MutexGuard};

use chatoms_application::{bootstrap::BootstrapStatus, error::ApplicationError};
use chatoms_domain::{Task, TaskId, TaskStateTransition};
use chatoms_infrastructure::bootstrap::{
    SharedDatabase, SharedFoundationRepository, SharedLoggingGuard, SharedResolvedAppPaths,
};
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, TimeProvider,
    error::PortFailure,
    repository::{ActiveLease, FoundationRepository, ProjectSummary, RepositoryError},
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
    resources: RuntimeResources,
}

impl AppRuntime {
    pub fn new(
        bootstrap_status: BootstrapStatus,
        repository: RepositoryHandle,
        time: TimeProviderHandle,
        capabilities: CapabilityHandle,
        resources: RuntimeResources,
    ) -> Self {
        Self {
            bootstrap_status,
            repository,
            time,
            capabilities,
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
