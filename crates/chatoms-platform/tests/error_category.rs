use chatoms_platform::PlatformError;
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    path::{PathError, PathErrorCode},
    permissions::{PermissionError, PermissionErrorCode, PermissionStatus},
};

#[test]
fn path_errors_map_to_platform_neutral_categories() {
    assert_eq!(
        PathError::new(PathErrorCode::EnvironmentUnavailable).category(),
        FailureCategory::StorageUnavailable
    );
    assert_eq!(
        PathError::new(PathErrorCode::ReparsePointRejected).category(),
        FailureCategory::StorageInsecure
    );
}

#[test]
fn permission_and_platform_errors_map_without_os_details() {
    assert_eq!(
        PermissionError::new(PermissionErrorCode::InsecureAcl).category(),
        FailureCategory::StorageInsecure
    );
    assert_eq!(
        PermissionError::new(PermissionErrorCode::UnsupportedPlatform).category(),
        FailureCategory::Unsupported
    );
    assert_eq!(
        PlatformError::PermissionNotSecure(PermissionStatus::Degraded).category(),
        FailureCategory::StorageInsecure
    );
}
