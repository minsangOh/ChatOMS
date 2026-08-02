use std::sync::{Arc, Mutex};

use chatoms_domain::{Task, TaskId, TaskStateTransition};
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState, LoggingBootstrapPort, LoggingBootstrapState,
    error::{CategorizedFailure, FailureCategory, PortFailure},
    path::ResolvedAppPaths,
    permissions::PermissionStatus,
    repository::{
        ActiveLease, FoundationRepository, ProjectSummary, RepositoryError, RepositoryErrorCode,
    },
};

use crate::{
    database::{DatabaseConnection, DatabaseError, MigrationRunner, SqliteFoundationRepository},
    logging::{LogLevel, LoggingConfig, LoggingGuard, ValidatedLogDirectory, initialize_logging},
};

pub type SharedResolvedAppPaths = Arc<Mutex<Option<ResolvedAppPaths>>>;

#[derive(Clone, Default)]
pub struct SharedDatabase {
    inner: Arc<Mutex<Option<DatabaseConnection>>>,
}

impl SharedDatabase {
    #[must_use]
    pub fn repository(&self) -> SharedFoundationRepository {
        SharedFoundationRepository {
            database: self.clone(),
        }
    }

    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.inner
            .lock()
            .map(|database| database.is_some())
            .unwrap_or(false)
    }
}

pub struct DatabaseBootstrapAdapter {
    paths: SharedResolvedAppPaths,
    database: SharedDatabase,
}

impl DatabaseBootstrapAdapter {
    #[must_use]
    pub const fn new(paths: SharedResolvedAppPaths, database: SharedDatabase) -> Self {
        Self { paths, database }
    }
}

impl DatabaseBootstrapPort for DatabaseBootstrapAdapter {
    fn bootstrap_database(&mut self) -> Result<DatabaseBootstrapState, PortFailure> {
        if self.database.is_initialized() {
            return Ok(DatabaseBootstrapState::Ready);
        }
        let database_path = self
            .paths
            .lock()
            .map_err(|_| internal_failure())?
            .as_ref()
            .map(|paths| paths.database_path.clone())
            .ok_or_else(storage_unavailable)?;
        let mut connection = DatabaseConnection::open(database_path).map_err(database_failure)?;
        let outcome = match MigrationRunner::default().run(&mut connection) {
            Ok(outcome) => outcome,
            Err(DatabaseError::DatabaseNewerThanApplication { .. }) => {
                return Ok(DatabaseBootstrapState::Incompatible);
            }
            Err(error) => return Err(database_failure(error)),
        };
        let status = if outcome.applied_count == 0 {
            DatabaseBootstrapState::Ready
        } else {
            DatabaseBootstrapState::Upgraded
        };
        let mut stored = self.database.inner.lock().map_err(|_| internal_failure())?;
        *stored = Some(connection);
        Ok(status)
    }
}

#[derive(Clone, Default)]
pub struct SharedLoggingGuard {
    inner: Arc<Mutex<Option<LoggingGuard>>>,
}

impl SharedLoggingGuard {
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

pub struct LoggingBootstrapAdapter {
    paths: SharedResolvedAppPaths,
    guard: SharedLoggingGuard,
}

impl LoggingBootstrapAdapter {
    #[must_use]
    pub const fn new(paths: SharedResolvedAppPaths, guard: SharedLoggingGuard) -> Self {
        Self { paths, guard }
    }
}

impl LoggingBootstrapPort for LoggingBootstrapAdapter {
    fn bootstrap_logging(&mut self) -> Result<LoggingBootstrapState, PortFailure> {
        if self.guard.is_initialized() {
            return Ok(LoggingBootstrapState::Ready);
        }
        let paths = self
            .paths
            .lock()
            .map_err(|_| internal_failure())?
            .clone()
            .ok_or_else(storage_unavailable)?;
        let directory = ValidatedLogDirectory::from_secure_paths(&paths, PermissionStatus::Secure)
            .map_err(categorized_failure)?;
        let guard = initialize_logging(&LoggingConfig::new(directory, LogLevel::Info))
            .map_err(categorized_failure)?;
        let mut stored = self.guard.inner.lock().map_err(|_| internal_failure())?;
        *stored = Some(guard);
        Ok(LoggingBootstrapState::Ready)
    }
}

#[derive(Clone)]
pub struct SharedFoundationRepository {
    database: SharedDatabase,
}

impl SharedFoundationRepository {
    fn with_repository<T>(
        &mut self,
        operation: impl FnOnce(&mut SqliteFoundationRepository<'_>) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut stored = self
            .database
            .inner
            .lock()
            .map_err(|_| RepositoryError::new(RepositoryErrorCode::DatabaseUnavailable))?;
        let database = stored
            .as_mut()
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::DatabaseUnavailable))?;
        operation(&mut SqliteFoundationRepository::new(database))
    }
}

impl FoundationRepository for SharedFoundationRepository {
    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.create_task(task, initial_transition, lease_acquired_at_ms)
        })
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.with_repository(|repository| repository.get_task(task_id))
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_transition(expected_version, task, transition)
        })
    }

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.save_recovery_target(expected_version, task))
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.terminate_task(expected_version, task, transition)
        })
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        self.with_repository(|repository| repository.list_task_transitions(task_id))
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.with_repository(|repository| repository.list_projects())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.with_repository(|repository| repository.active_lease())
    }
}

fn categorized_failure(error: impl CategorizedFailure) -> PortFailure {
    PortFailure::with_policy(error.category(), error.severity(), error.retry())
}

fn database_failure(error: DatabaseError) -> PortFailure {
    categorized_failure(error)
}

fn storage_unavailable() -> PortFailure {
    PortFailure::new(FailureCategory::StorageUnavailable)
}

fn internal_failure() -> PortFailure {
    PortFailure::new(FailureCategory::Internal)
}
