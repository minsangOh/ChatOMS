use std::error::Error as _;

use chatoms_infrastructure::{
    database::DatabaseError,
    error::{InfrastructureError, InfrastructureErrorCode},
    logging::LoggingError,
    redaction::RedactionError,
};
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    repository::{RepositoryError, RepositoryErrorCode},
};

#[test]
fn repository_and_database_errors_map_without_exposing_sources() {
    let repository = RepositoryError::new(RepositoryErrorCode::ActiveLeaseConflict);
    let unavailable = DatabaseError::OpenDatabase(rusqlite::Error::InvalidQuery);
    let migration = DatabaseError::MigrationChecksumMismatch { version: 1 };
    let invariant = DatabaseError::ForeignKeyViolation {
        version: 1,
        violations: 1,
    };
    let cases: Vec<(&dyn CategorizedFailure, FailureCategory)> = vec![
        (&repository, FailureCategory::ActiveLeaseConflict),
        (&unavailable, FailureCategory::StorageUnavailable),
        (&migration, FailureCategory::MigrationFailure),
        (&invariant, FailureCategory::InvariantViolation),
    ];
    for (error, expected) in cases {
        assert_eq!(error.category(), expected);
        assert!(!error.category().as_str().contains("secret"));
    }
}

#[test]
fn redaction_logging_and_wrapper_errors_have_stable_categories() {
    assert_eq!(
        RedactionError::UnsafeOutput.category(),
        FailureCategory::RedactionFailure
    );
    assert_eq!(
        LoggingError::InvalidConfiguration.category(),
        FailureCategory::LoggingFailure
    );

    let wrapper = InfrastructureError::with_source(
        InfrastructureErrorCode::Database,
        std::io::Error::other("C:\\private\\secret.db"),
    );
    assert_eq!(wrapper.category(), FailureCategory::StorageUnavailable);
    assert!(wrapper.source().is_some());
    assert!(!wrapper.to_string().contains("C:\\private"));
    assert!(!wrapper.to_string().contains("secret.db"));
}
