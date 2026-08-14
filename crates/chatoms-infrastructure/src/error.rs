use std::{error::Error, fmt};

use chatoms_ports::error::{
    CategorizedFailure, FailureCategory, FailureSeverity, RetryDisposition,
};

use crate::database::DatabaseError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InfrastructureErrorCode {
    Database,
    Migration,
    Repository,
    Redaction,
    Logging,
    Invariant,
    Internal,
}

#[derive(Debug)]
pub struct InfrastructureError {
    code: InfrastructureErrorCode,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl InfrastructureError {
    #[must_use]
    pub const fn new(code: InfrastructureErrorCode) -> Self {
        Self { code, source: None }
    }

    #[must_use]
    pub fn with_source(
        code: InfrastructureErrorCode,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }

    #[must_use]
    pub const fn code(&self) -> InfrastructureErrorCode {
        self.code
    }
}

impl fmt::Display for InfrastructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "infrastructure operation failed: {:?}",
            self.code
        )
    }
}

impl Error for InfrastructureError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

impl CategorizedFailure for InfrastructureError {
    fn category(&self) -> FailureCategory {
        match self.code {
            InfrastructureErrorCode::Database => FailureCategory::StorageUnavailable,
            InfrastructureErrorCode::Migration => FailureCategory::MigrationFailure,
            InfrastructureErrorCode::Repository => FailureCategory::Internal,
            InfrastructureErrorCode::Redaction => FailureCategory::RedactionFailure,
            InfrastructureErrorCode::Logging => FailureCategory::LoggingFailure,
            InfrastructureErrorCode::Invariant => FailureCategory::InvariantViolation,
            InfrastructureErrorCode::Internal => FailureCategory::Internal,
        }
    }
}

impl CategorizedFailure for DatabaseError {
    fn category(&self) -> FailureCategory {
        match self {
            Self::OpenDatabase(_) | Self::ConfigureDatabase { .. } | Self::VerifyPragma { .. } => {
                FailureCategory::StorageUnavailable
            }
            Self::MigrationRegistryInvalid { .. }
            | Self::MigrationMetadataInvalid { .. }
            | Self::MigrationChecksumMismatch { .. }
            | Self::MigrationOutOfOrder { .. }
            | Self::DatabaseNewerThanApplication { .. }
            | Self::MigrationExecutionFailed { .. }
            | Self::LegacyProjectPreflightFailed { .. }
            | Self::ValidationCommandApprovalMigrationFailed { .. }
            | Self::ValidationCommandEnvironmentBindingMigrationFailed { .. } => {
                FailureCategory::MigrationFailure
            }
            Self::ForeignKeyViolation { .. } | Self::InvariantViolation { .. } => {
                FailureCategory::InvariantViolation
            }
        }
    }

    fn severity(&self) -> FailureSeverity {
        self.category().default_severity()
    }

    fn retry(&self) -> RetryDisposition {
        self.category().default_retry()
    }
}
