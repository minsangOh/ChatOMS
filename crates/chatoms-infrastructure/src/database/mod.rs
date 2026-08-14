mod connection;
mod migration;
mod repository;
mod schema;

use thiserror::Error;

pub use connection::{DatabaseConnection, PragmaSettings};
pub use migration::{
    FOUNDATION_MIGRATION, LegacyProject, LegacyProjectIdentity, LegacyProjectPreflight, Migration,
    MigrationOutcome, MigrationRunner, checksum_sha256, validate_registry,
};
pub use repository::SqliteFoundationRepository;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database could not be opened")]
    OpenDatabase(#[source] rusqlite::Error),
    #[error("database connection configuration failed for {pragma}")]
    ConfigureDatabase {
        pragma: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("database PRAGMA verification failed for {pragma}")]
    VerifyPragma {
        pragma: &'static str,
        expected: String,
        actual: String,
    },
    #[error("migration registry is invalid: {reason}")]
    MigrationRegistryInvalid { reason: &'static str },
    #[error("migration metadata is invalid: {reason}")]
    MigrationMetadataInvalid {
        reason: &'static str,
        #[source]
        source: Option<rusqlite::Error>,
    },
    #[error("migration checksum mismatch at version {version}")]
    MigrationChecksumMismatch { version: u32 },
    #[error("migration history is out of order: expected {expected}, found {found}")]
    MigrationOutOfOrder { expected: u32, found: u32 },
    #[error(
        "database schema version {database_version} is newer than application version {application_version}"
    )]
    DatabaseNewerThanApplication {
        database_version: u32,
        application_version: u32,
    },
    #[error("migration {version} ({name}) execution failed")]
    MigrationExecutionFailed {
        version: u32,
        name: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "legacy project migration preflight failed for project {project_id} ({display_path}): {reason}"
    )]
    LegacyProjectPreflightFailed {
        project_id: String,
        display_path: String,
        reason: &'static str,
    },
    #[error("migration {version} produced {violations} foreign-key violations")]
    ForeignKeyViolation { version: u32, violations: usize },
    #[error("database invariant violation: {reason}")]
    InvariantViolation { reason: &'static str },
    #[error("migration 11 (validation_command_executable_binding) aborted: {reason}")]
    ValidationCommandApprovalMigrationFailed { reason: &'static str },
    #[error("migration 12 (validation_command_environment_binding) aborted: {reason}")]
    ValidationCommandEnvironmentBindingMigrationFailed { reason: &'static str },
}
