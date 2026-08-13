use std::fmt;

use chatoms_domain::DomainError;
use chatoms_ports::error::{
    CategorizedFailure, FailureCategory, FailureSeverity, RetryDisposition,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationErrorCode {
    InvalidInput,
    InvalidState,
    NotFound,
    AlreadyExists,
    Conflict,
    VersionConflict,
    SequenceConflict,
    ActiveTaskConflict,
    StorageUnavailable,
    StorageInsecure,
    PermissionDenied,
    MigrationFailed,
    RedactionFailed,
    LoggingUnavailable,
    Unsupported,
    Internal,
}

impl ApplicationErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "APP_INVALID_INPUT",
            Self::InvalidState => "APP_INVALID_STATE",
            Self::NotFound => "APP_NOT_FOUND",
            Self::AlreadyExists => "APP_ALREADY_EXISTS",
            Self::Conflict => "APP_CONFLICT",
            Self::VersionConflict => "APP_VERSION_CONFLICT",
            Self::SequenceConflict => "APP_SEQUENCE_CONFLICT",
            Self::ActiveTaskConflict => "APP_ACTIVE_TASK_CONFLICT",
            Self::StorageUnavailable => "APP_STORAGE_UNAVAILABLE",
            Self::StorageInsecure => "APP_STORAGE_INSECURE",
            Self::PermissionDenied => "APP_PERMISSION_DENIED",
            Self::MigrationFailed => "APP_MIGRATION_FAILED",
            Self::RedactionFailed => "APP_REDACTION_FAILED",
            Self::LoggingUnavailable => "APP_LOGGING_UNAVAILABLE",
            Self::Unsupported => "APP_UNSUPPORTED",
            Self::Internal => "APP_INTERNAL",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationError {
    code: ApplicationErrorCode,
    user_message: &'static str,
    severity: FailureSeverity,
    retry: RetryDisposition,
}

impl ApplicationError {
    #[must_use]
    pub fn from_categorized(error: &impl CategorizedFailure) -> Self {
        Self::from_failure(error.category(), error.severity(), error.retry())
    }

    #[must_use]
    pub const fn from_failure(
        category: FailureCategory,
        severity: FailureSeverity,
        retry: RetryDisposition,
    ) -> Self {
        let (code, user_message) = mapping(category);
        Self {
            code,
            user_message,
            severity,
            retry,
        }
    }

    #[must_use]
    pub fn from_domain(error: &DomainError) -> Self {
        let category = match error {
            DomainError::InvalidTaskState | DomainError::InvalidStateTransition => {
                FailureCategory::InvalidState
            }
            DomainError::InvariantViolation => FailureCategory::InvariantViolation,
            DomainError::InvalidUuid
            | DomainError::UnsupportedUuidVersion
            | DomainError::InvalidTaskBranchIdentity
            | DomainError::InvalidTimestamp
            | DomainError::InvalidVersion
            | DomainError::InvalidActorKind
            | DomainError::InvalidReasonCode
            | DomainError::InvalidTaskBrief => FailureCategory::InvalidInput,
        };
        Self::from_failure(
            category,
            category.default_severity(),
            category.default_retry(),
        )
    }

    #[must_use]
    pub const fn code(&self) -> ApplicationErrorCode {
        self.code
    }

    #[must_use]
    pub const fn user_message(&self) -> &'static str {
        self.user_message
    }

    #[must_use]
    pub const fn severity(&self) -> FailureSeverity {
        self.severity
    }

    #[must_use]
    pub const fn retry(&self) -> RetryDisposition {
        self.retry
    }
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.user_message)
    }
}

const fn mapping(category: FailureCategory) -> (ApplicationErrorCode, &'static str) {
    match category {
        FailureCategory::InvalidInput => (
            ApplicationErrorCode::InvalidInput,
            "The supplied data is invalid.",
        ),
        FailureCategory::InvalidState => (
            ApplicationErrorCode::InvalidState,
            "The operation is not valid in the current state.",
        ),
        FailureCategory::NotFound => (
            ApplicationErrorCode::NotFound,
            "The requested item could not be found.",
        ),
        FailureCategory::AlreadyExists => (
            ApplicationErrorCode::AlreadyExists,
            "The item already exists.",
        ),
        FailureCategory::Conflict => (
            ApplicationErrorCode::Conflict,
            "The operation conflicts with the current state.",
        ),
        FailureCategory::VersionConflict => (
            ApplicationErrorCode::VersionConflict,
            "The task changed and must be refreshed before retrying.",
        ),
        FailureCategory::SequenceConflict => (
            ApplicationErrorCode::SequenceConflict,
            "The task history changed and must be refreshed.",
        ),
        FailureCategory::ActiveLeaseConflict => (
            ApplicationErrorCode::ActiveTaskConflict,
            "Another task is already active.",
        ),
        FailureCategory::StorageUnavailable => (
            ApplicationErrorCode::StorageUnavailable,
            "Secure local storage is unavailable.",
        ),
        FailureCategory::StorageInsecure => (
            ApplicationErrorCode::StorageInsecure,
            "Local storage does not meet the required security policy.",
        ),
        FailureCategory::PermissionDenied => (
            ApplicationErrorCode::PermissionDenied,
            "The application cannot access its secure local storage.",
        ),
        FailureCategory::MigrationFailure => (
            ApplicationErrorCode::MigrationFailed,
            "The local database could not be upgraded safely.",
        ),
        FailureCategory::RedactionFailure => (
            ApplicationErrorCode::RedactionFailed,
            "Sensitive information could not be processed safely.",
        ),
        FailureCategory::LoggingFailure => (
            ApplicationErrorCode::LoggingUnavailable,
            "Local diagnostic logging is unavailable.",
        ),
        FailureCategory::Unsupported => (
            ApplicationErrorCode::Unsupported,
            "This operation is not supported on the current platform.",
        ),
        FailureCategory::InvariantViolation | FailureCategory::Internal => (
            ApplicationErrorCode::Internal,
            "An internal error occurred.",
        ),
    }
}
