use chatoms_domain::TaskId;
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState, LoggingBootstrapPort, LoggingBootstrapState,
    StorageBootstrapPort, StorageBootstrapState,
    error::CategorizedFailure,
    repository::{ActiveLease, FoundationRepository},
};

use crate::{APPLICATION_VERSION, error::ApplicationError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Ready,
    Unavailable,
    Insecure,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseStatus {
    NotChecked,
    Ready,
    Upgraded,
    MigrationRequired,
    Unavailable,
    Incompatible,
}

impl DatabaseStatus {
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::Upgraded)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoggingStatus {
    NotChecked,
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveTaskStatus {
    NotChecked,
    None,
    Active {
        task_id: TaskId,
        acquired_at_ms: i64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapStatus {
    pub storage_status: StorageStatus,
    pub database_status: DatabaseStatus,
    pub logging_status: LoggingStatus,
    pub active_task_status: ActiveTaskStatus,
    pub application_version: &'static str,
    pub ready: bool,
}

pub struct BootstrapService<'a, S, D, L, R> {
    storage: &'a mut S,
    database: &'a mut D,
    logging: &'a mut L,
    repository: &'a mut R,
}

impl<'a, S, D, L, R> BootstrapService<'a, S, D, L, R>
where
    S: StorageBootstrapPort,
    D: DatabaseBootstrapPort,
    L: LoggingBootstrapPort,
    R: FoundationRepository,
{
    pub fn new(
        storage: &'a mut S,
        database: &'a mut D,
        logging: &'a mut L,
        repository: &'a mut R,
    ) -> Self {
        Self {
            storage,
            database,
            logging,
            repository,
        }
    }

    pub fn bootstrap(&mut self) -> Result<BootstrapStatus, ApplicationError> {
        let storage_status = self
            .storage
            .prepare_secure_storage()
            .map(StorageStatus::from)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if storage_status != StorageStatus::Ready {
            return Ok(partial_status(storage_status));
        }

        let database_status = self
            .database
            .bootstrap_database()
            .map(DatabaseStatus::from)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if !database_status.is_ready() {
            return Ok(BootstrapStatus {
                storage_status,
                database_status,
                logging_status: LoggingStatus::NotChecked,
                active_task_status: ActiveTaskStatus::NotChecked,
                application_version: APPLICATION_VERSION,
                ready: false,
            });
        }

        let logging_status = match self.logging.bootstrap_logging() {
            Ok(status) => LoggingStatus::from(status),
            Err(error)
                if error.category() == chatoms_ports::error::FailureCategory::LoggingFailure =>
            {
                LoggingStatus::Unavailable
            }
            Err(error) => return Err(ApplicationError::from_categorized(&error)),
        };
        let active_task_status = self
            .repository
            .active_lease()
            .map(ActiveTaskStatus::from)
            .map_err(|error| ApplicationError::from_categorized(&error))?;

        Ok(BootstrapStatus {
            storage_status,
            database_status,
            logging_status,
            active_task_status,
            application_version: APPLICATION_VERSION,
            ready: true,
        })
    }
}

fn partial_status(storage_status: StorageStatus) -> BootstrapStatus {
    BootstrapStatus {
        storage_status,
        database_status: DatabaseStatus::NotChecked,
        logging_status: LoggingStatus::NotChecked,
        active_task_status: ActiveTaskStatus::NotChecked,
        application_version: APPLICATION_VERSION,
        ready: false,
    }
}

impl From<StorageBootstrapState> for StorageStatus {
    fn from(value: StorageBootstrapState) -> Self {
        match value {
            StorageBootstrapState::Ready => Self::Ready,
            StorageBootstrapState::Unavailable => Self::Unavailable,
            StorageBootstrapState::Insecure => Self::Insecure,
            StorageBootstrapState::Unsupported => Self::Unsupported,
        }
    }
}

impl From<DatabaseBootstrapState> for DatabaseStatus {
    fn from(value: DatabaseBootstrapState) -> Self {
        match value {
            DatabaseBootstrapState::Ready => Self::Ready,
            DatabaseBootstrapState::Upgraded => Self::Upgraded,
            DatabaseBootstrapState::MigrationRequired => Self::MigrationRequired,
            DatabaseBootstrapState::Unavailable => Self::Unavailable,
            DatabaseBootstrapState::Incompatible => Self::Incompatible,
        }
    }
}

impl From<LoggingBootstrapState> for LoggingStatus {
    fn from(value: LoggingBootstrapState) -> Self {
        match value {
            LoggingBootstrapState::Ready => Self::Ready,
            LoggingBootstrapState::Unavailable => Self::Unavailable,
        }
    }
}

impl From<Option<ActiveLease>> for ActiveTaskStatus {
    fn from(value: Option<ActiveLease>) -> Self {
        value.map_or(Self::None, |lease| Self::Active {
            task_id: lease.task_id,
            acquired_at_ms: lease.acquired_at_ms,
        })
    }
}
