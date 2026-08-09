use std::fmt;

use crate::{
    path::{PathError, PathErrorCode},
    permissions::{PermissionError, PermissionErrorCode},
    repository::{RepositoryError, RepositoryErrorCode},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FailureCategory {
    InvalidInput,
    InvalidState,
    NotFound,
    AlreadyExists,
    Conflict,
    VersionConflict,
    SequenceConflict,
    ActiveLeaseConflict,
    StorageUnavailable,
    StorageInsecure,
    PermissionDenied,
    MigrationFailure,
    RedactionFailure,
    LoggingFailure,
    Unsupported,
    InvariantViolation,
    Internal,
}

impl FailureCategory {
    pub const ALL: [Self; 17] = [
        Self::InvalidInput,
        Self::InvalidState,
        Self::NotFound,
        Self::AlreadyExists,
        Self::Conflict,
        Self::VersionConflict,
        Self::SequenceConflict,
        Self::ActiveLeaseConflict,
        Self::StorageUnavailable,
        Self::StorageInsecure,
        Self::PermissionDenied,
        Self::MigrationFailure,
        Self::RedactionFailure,
        Self::LoggingFailure,
        Self::Unsupported,
        Self::InvariantViolation,
        Self::Internal,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "FAILURE_INVALID_INPUT",
            Self::InvalidState => "FAILURE_INVALID_STATE",
            Self::NotFound => "FAILURE_NOT_FOUND",
            Self::AlreadyExists => "FAILURE_ALREADY_EXISTS",
            Self::Conflict => "FAILURE_CONFLICT",
            Self::VersionConflict => "FAILURE_VERSION_CONFLICT",
            Self::SequenceConflict => "FAILURE_SEQUENCE_CONFLICT",
            Self::ActiveLeaseConflict => "FAILURE_ACTIVE_LEASE_CONFLICT",
            Self::StorageUnavailable => "FAILURE_STORAGE_UNAVAILABLE",
            Self::StorageInsecure => "FAILURE_STORAGE_INSECURE",
            Self::PermissionDenied => "FAILURE_PERMISSION_DENIED",
            Self::MigrationFailure => "FAILURE_MIGRATION",
            Self::RedactionFailure => "FAILURE_REDACTION",
            Self::LoggingFailure => "FAILURE_LOGGING",
            Self::Unsupported => "FAILURE_UNSUPPORTED",
            Self::InvariantViolation => "FAILURE_INVARIANT",
            Self::Internal => "FAILURE_INTERNAL",
        }
    }

    #[must_use]
    pub const fn default_severity(self) -> FailureSeverity {
        match self {
            Self::InvalidInput
            | Self::InvalidState
            | Self::NotFound
            | Self::AlreadyExists
            | Self::Conflict
            | Self::VersionConflict
            | Self::SequenceConflict
            | Self::ActiveLeaseConflict
            | Self::Unsupported => FailureSeverity::Warning,
            Self::StorageUnavailable
            | Self::PermissionDenied
            | Self::MigrationFailure
            | Self::LoggingFailure => FailureSeverity::Error,
            Self::StorageInsecure
            | Self::RedactionFailure
            | Self::InvariantViolation
            | Self::Internal => FailureSeverity::Critical,
        }
    }

    #[must_use]
    pub const fn default_retry(self) -> RetryDisposition {
        match self {
            Self::VersionConflict | Self::SequenceConflict => RetryDisposition::AfterStateRefresh,
            Self::ActiveLeaseConflict
            | Self::StorageUnavailable
            | Self::StorageInsecure
            | Self::PermissionDenied
            | Self::MigrationFailure
            | Self::LoggingFailure
            | Self::Unsupported => RetryDisposition::AfterUserAction,
            Self::Conflict => RetryDisposition::Immediate,
            Self::InvalidInput
            | Self::InvalidState
            | Self::NotFound
            | Self::AlreadyExists
            | Self::RedactionFailure
            | Self::InvariantViolation
            | Self::Internal => RetryDisposition::Never,
        }
    }
}

impl fmt::Display for FailureCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl FailureSeverity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
            Self::Critical => "CRITICAL",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDisposition {
    Never,
    Immediate,
    AfterUserAction,
    AfterStateRefresh,
}

impl RetryDisposition {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "NEVER",
            Self::Immediate => "IMMEDIATE",
            Self::AfterUserAction => "AFTER_USER_ACTION",
            Self::AfterStateRefresh => "AFTER_STATE_REFRESH",
        }
    }
}

pub trait CategorizedFailure {
    fn category(&self) -> FailureCategory;

    fn severity(&self) -> FailureSeverity {
        self.category().default_severity()
    }

    fn retry(&self) -> RetryDisposition {
        self.category().default_retry()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortFailure {
    category: FailureCategory,
    severity: FailureSeverity,
    retry: RetryDisposition,
}

impl PortFailure {
    #[must_use]
    pub const fn new(category: FailureCategory) -> Self {
        Self {
            category,
            severity: category.default_severity(),
            retry: category.default_retry(),
        }
    }

    #[must_use]
    pub const fn with_policy(
        category: FailureCategory,
        severity: FailureSeverity,
        retry: RetryDisposition,
    ) -> Self {
        Self {
            category,
            severity,
            retry,
        }
    }
}

impl fmt::Display for PortFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.category.as_str())
    }
}

impl std::error::Error for PortFailure {}

impl CategorizedFailure for PortFailure {
    fn category(&self) -> FailureCategory {
        self.category
    }

    fn severity(&self) -> FailureSeverity {
        self.severity
    }

    fn retry(&self) -> RetryDisposition {
        self.retry
    }
}

impl CategorizedFailure for RepositoryError {
    fn category(&self) -> FailureCategory {
        match self.code() {
            RepositoryErrorCode::ProjectNotFound
            | RepositoryErrorCode::TaskNotFound
            | RepositoryErrorCode::IsolationNotFound => FailureCategory::NotFound,
            RepositoryErrorCode::DuplicateProject
            | RepositoryErrorCode::DuplicateTask
            | RepositoryErrorCode::DuplicateIsolation => FailureCategory::AlreadyExists,
            RepositoryErrorCode::VersionConflict => FailureCategory::VersionConflict,
            RepositoryErrorCode::TransitionSequenceConflict => FailureCategory::SequenceConflict,
            RepositoryErrorCode::ActiveLeaseConflict => FailureCategory::ActiveLeaseConflict,
            RepositoryErrorCode::InvalidAggregate => FailureCategory::InvalidInput,
            RepositoryErrorCode::InvalidPersistenceState => FailureCategory::InvariantViolation,
            RepositoryErrorCode::DatabaseUnavailable => FailureCategory::StorageUnavailable,
            RepositoryErrorCode::OperationFailed => FailureCategory::Internal,
        }
    }
}

impl CategorizedFailure for PathError {
    fn category(&self) -> FailureCategory {
        match self.code() {
            PathErrorCode::EnvironmentUnavailable | PathErrorCode::CreateDirectoryFailed => {
                FailureCategory::StorageUnavailable
            }
            PathErrorCode::InvalidBasePath
            | PathErrorCode::RelativeBasePath
            | PathErrorCode::PathOutsideRoot
            | PathErrorCode::InvalidTaskPath => FailureCategory::InvalidInput,
            PathErrorCode::PathOccupiedByFile => FailureCategory::AlreadyExists,
            PathErrorCode::ReparsePointRejected => FailureCategory::StorageInsecure,
        }
    }
}

impl CategorizedFailure for PermissionError {
    fn category(&self) -> FailureCategory {
        match self.code() {
            PermissionErrorCode::CurrentUserSidUnavailable
            | PermissionErrorCode::ReadAclFailed
            | PermissionErrorCode::WriteAclFailed
            | PermissionErrorCode::PermissionDenied => FailureCategory::PermissionDenied,
            PermissionErrorCode::VerifyAclFailed | PermissionErrorCode::InsecureAcl => {
                FailureCategory::StorageInsecure
            }
            PermissionErrorCode::UnsupportedPlatform => FailureCategory::Unsupported,
            PermissionErrorCode::InvariantViolation => FailureCategory::InvariantViolation,
        }
    }
}
