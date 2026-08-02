use std::collections::HashSet;

use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory, FailureSeverity, PortFailure, RetryDisposition},
    repository::{RepositoryError, RepositoryErrorCode},
};

struct SyntheticFailure;

impl CategorizedFailure for SyntheticFailure {
    fn category(&self) -> FailureCategory {
        FailureCategory::StorageInsecure
    }

    fn severity(&self) -> FailureSeverity {
        FailureSeverity::Critical
    }

    fn retry(&self) -> RetryDisposition {
        RetryDisposition::AfterUserAction
    }
}

#[test]
fn category_codes_are_stable_unique_and_non_sensitive() {
    let codes = FailureCategory::ALL
        .into_iter()
        .map(FailureCategory::as_str)
        .collect::<Vec<_>>();
    assert_eq!(codes.len(), 17);
    assert_eq!(codes.iter().copied().collect::<HashSet<_>>().len(), 17);
    assert_eq!(
        FailureCategory::VersionConflict.as_str(),
        "FAILURE_VERSION_CONFLICT"
    );
    assert_eq!(
        FailureCategory::ActiveLeaseConflict.as_str(),
        "FAILURE_ACTIVE_LEASE_CONFLICT"
    );
    for code in codes {
        assert!(code.starts_with("FAILURE_"));
        assert!(!code.contains(['\\', '/', ':']));
        assert!(!code.to_ascii_lowercase().contains("secret"));
    }
}

#[test]
fn severity_and_retry_codes_are_stable() {
    assert_eq!(FailureSeverity::Info.as_str(), "INFO");
    assert_eq!(FailureSeverity::Warning.as_str(), "WARNING");
    assert_eq!(FailureSeverity::Error.as_str(), "ERROR");
    assert_eq!(FailureSeverity::Critical.as_str(), "CRITICAL");
    assert_eq!(RetryDisposition::Never.as_str(), "NEVER");
    assert_eq!(RetryDisposition::Immediate.as_str(), "IMMEDIATE");
    assert_eq!(
        RetryDisposition::AfterUserAction.as_str(),
        "AFTER_USER_ACTION"
    );
    assert_eq!(
        RetryDisposition::AfterStateRefresh.as_str(),
        "AFTER_STATE_REFRESH"
    );
}

#[test]
fn categorized_contract_uses_category_defaults_without_source_text() {
    let error = RepositoryError::new(RepositoryErrorCode::VersionConflict);
    assert_eq!(error.category(), FailureCategory::VersionConflict);
    assert_eq!(error.severity(), FailureSeverity::Warning);
    assert_eq!(error.retry(), RetryDisposition::AfterStateRefresh);
    assert!(!format!("{}", error.category()).contains("C:\\Users"));
}

#[test]
fn synthetic_failure_can_override_category_defaults_without_adapter_types() {
    let error = SyntheticFailure;
    assert_eq!(error.category(), FailureCategory::StorageInsecure);
    assert_eq!(error.severity(), FailureSeverity::Critical);
    assert_eq!(error.retry(), RetryDisposition::AfterUserAction);
}

#[test]
fn neutral_port_failure_preserves_explicit_policy_without_a_source_chain() {
    let failure = PortFailure::with_policy(
        FailureCategory::LoggingFailure,
        FailureSeverity::Critical,
        RetryDisposition::Never,
    );
    assert_eq!(failure.category(), FailureCategory::LoggingFailure);
    assert_eq!(failure.severity(), FailureSeverity::Critical);
    assert_eq!(failure.retry(), RetryDisposition::Never);
    assert_eq!(failure.to_string(), "FAILURE_LOGGING");
}
