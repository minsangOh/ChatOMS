use chatoms_application::error::{ApplicationError, ApplicationErrorCode};
use chatoms_domain::DomainError;
use chatoms_ports::error::{
    CategorizedFailure, FailureCategory, FailureSeverity, RetryDisposition,
};

struct SyntheticFailure {
    category: FailureCategory,
}

impl CategorizedFailure for SyntheticFailure {
    fn category(&self) -> FailureCategory {
        self.category
    }
}

#[test]
fn every_category_maps_to_a_stable_safe_application_code_and_message() {
    for category in FailureCategory::ALL {
        let mapped = ApplicationError::from_categorized(&SyntheticFailure { category });
        assert!(mapped.code().as_str().starts_with("APP_"));
        assert!(!mapped.user_message().is_empty());
        assert_eq!(mapped.severity(), category.default_severity());
        assert_eq!(mapped.retry(), category.default_retry());
        let public = mapped.to_string();
        for forbidden in ["C:\\Users", "S-1-5-", "SELECT ", "secret-value"] {
            assert!(!public.contains(forbidden));
        }
    }
}

#[test]
fn important_categories_have_the_approved_codes_and_retry_policy() {
    let cases = [
        (
            FailureCategory::InvalidInput,
            ApplicationErrorCode::InvalidInput,
        ),
        (
            FailureCategory::InvalidState,
            ApplicationErrorCode::InvalidState,
        ),
        (FailureCategory::NotFound, ApplicationErrorCode::NotFound),
        (
            FailureCategory::VersionConflict,
            ApplicationErrorCode::VersionConflict,
        ),
        (
            FailureCategory::SequenceConflict,
            ApplicationErrorCode::SequenceConflict,
        ),
        (
            FailureCategory::ActiveLeaseConflict,
            ApplicationErrorCode::ActiveTaskConflict,
        ),
        (
            FailureCategory::StorageUnavailable,
            ApplicationErrorCode::StorageUnavailable,
        ),
        (
            FailureCategory::StorageInsecure,
            ApplicationErrorCode::StorageInsecure,
        ),
        (
            FailureCategory::PermissionDenied,
            ApplicationErrorCode::PermissionDenied,
        ),
        (
            FailureCategory::MigrationFailure,
            ApplicationErrorCode::MigrationFailed,
        ),
        (
            FailureCategory::RedactionFailure,
            ApplicationErrorCode::RedactionFailed,
        ),
        (
            FailureCategory::LoggingFailure,
            ApplicationErrorCode::LoggingUnavailable,
        ),
        (
            FailureCategory::Unsupported,
            ApplicationErrorCode::Unsupported,
        ),
        (FailureCategory::Internal, ApplicationErrorCode::Internal),
    ];
    for (category, expected_code) in cases {
        let mapped = ApplicationError::from_categorized(&SyntheticFailure { category });
        assert_eq!(mapped.code(), expected_code);
    }
    let version = ApplicationError::from_categorized(&SyntheticFailure {
        category: FailureCategory::VersionConflict,
    });
    assert_eq!(version.retry(), RetryDisposition::AfterStateRefresh);
    assert_eq!(version.severity(), FailureSeverity::Warning);
}

#[test]
fn explicit_mapping_preserves_contract_policy_without_a_concrete_error() {
    let mapped = ApplicationError::from_failure(
        FailureCategory::StorageUnavailable,
        FailureSeverity::Critical,
        RetryDisposition::Immediate,
    );
    assert_eq!(mapped.code(), ApplicationErrorCode::StorageUnavailable);
    assert_eq!(mapped.severity(), FailureSeverity::Critical);
    assert_eq!(mapped.retry(), RetryDisposition::Immediate);
}

#[test]
fn domain_invalid_transition_maps_without_changing_the_domain_crate() {
    let mapped = ApplicationError::from_domain(&DomainError::InvalidStateTransition);
    assert_eq!(mapped.code(), ApplicationErrorCode::InvalidState);
    assert_eq!(
        mapped.user_message(),
        "The operation is not valid in the current state."
    );
}
