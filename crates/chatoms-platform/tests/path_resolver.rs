use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use chatoms_platform::{PlatformError, SecureAppPaths, path::WindowsPathResolver};
use chatoms_ports::{
    path::{
        APP_DIRECTORY_NAME, APP_IDENTIFIER, AppPathResolver, DATABASE_FILE_NAME, PathErrorCode,
        TaskId,
    },
    permissions::{
        FilesystemPermissionManager, PermissionError, PermissionErrorCode, PermissionStatus,
    },
};

struct AlwaysSecure;

impl FilesystemPermissionManager for AlwaysSecure {
    fn secure_directory(&self, _path: &Path) -> Result<(), PermissionError> {
        Ok(())
    }
    fn verify_directory(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(PermissionStatus::Secure)
    }
    fn secure_file(&self, _path: &Path) -> Result<(), PermissionError> {
        Ok(())
    }
    fn verify_file(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(PermissionStatus::Secure)
    }
}

struct RejectLogs;

impl FilesystemPermissionManager for RejectLogs {
    fn secure_directory(&self, path: &Path) -> Result<(), PermissionError> {
        if path.file_name().is_some_and(|name| name == "logs") {
            Err(PermissionError::new(PermissionErrorCode::WriteAclFailed))
        } else {
            Ok(())
        }
    }
    fn verify_directory(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(PermissionStatus::Secure)
    }
    fn secure_file(&self, _path: &Path) -> Result<(), PermissionError> {
        Ok(())
    }
    fn verify_file(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(PermissionStatus::Secure)
    }
}

#[test]
fn absolute_base_produces_deterministic_separated_layout() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("absolute local base");
    let first = resolver.validate_layout().expect("valid layout");
    let second = resolver.validate_layout().expect("deterministic layout");
    let root = temp.path().join(APP_DIRECTORY_NAME);

    assert_eq!(APP_IDENTIFIER, "io.github.minsangoh.chatoms");
    assert_eq!(first, second);
    assert_eq!(first.app_root, root);
    assert_eq!(first.data_dir, root.join("data"));
    assert_eq!(
        first.database_path,
        root.join("data").join(DATABASE_FILE_NAME)
    );
    assert_eq!(first.logs_dir, root.join("logs"));
    assert_eq!(first.artifacts_dir, root.join("artifacts"));
    assert_eq!(first.temp_dir, root.join("temp"));
    assert!(first.app_root.is_absolute());
}

#[test]
fn relative_empty_and_unc_bases_are_rejected() {
    for (path, expected) in [
        (PathBuf::new(), PathErrorCode::InvalidBasePath),
        (PathBuf::from("relative"), PathErrorCode::RelativeBasePath),
        (
            PathBuf::from(r"\\server\share"),
            PathErrorCode::InvalidBasePath,
        ),
    ] {
        let error = WindowsPathResolver::from_base_dir_for_test(path)
            .expect_err("invalid base must be rejected");
        assert_eq!(error.code(), expected);
    }
}

#[test]
fn task_paths_use_only_canonical_uuid_below_expected_parent() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("absolute local base");
    let task_id =
        TaskId::from_str("0197f9e2-7f0d-7d23-9db8-9447331a3810").expect("canonical UUIDv7");
    let artifact = resolver.task_artifact_dir(task_id).expect("artifact path");
    let temporary = resolver.task_temp_dir(task_id).expect("temporary path");
    let component = task_id.to_string();

    assert_eq!(artifact.file_name(), Some(component.as_ref()));
    assert_eq!(temporary.file_name(), Some(component.as_ref()));
    assert!(artifact.starts_with(resolver.artifacts_dir().expect("artifacts parent")));
    assert!(temporary.starts_with(resolver.temp_dir().expect("temp parent")));
    assert!(!component.contains(['/', '\\']));
}

#[test]
fn secure_prepare_is_idempotent_and_does_not_create_database_or_task_directories() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("absolute local base");
    let task_id = TaskId::new();
    let task_artifact = resolver
        .task_artifact_dir(task_id)
        .expect("task artifact path");

    let first = SecureAppPaths::prepare(&resolver, &AlwaysSecure).expect("first prepare");
    let second = SecureAppPaths::prepare(&resolver, &AlwaysSecure).expect("second prepare");

    assert_eq!(first, second);
    for directory in [
        &first.app_root,
        &first.data_dir,
        &first.logs_dir,
        &first.artifacts_dir,
        &first.temp_dir,
    ] {
        assert!(directory.is_dir());
    }
    assert!(!first.database_path.exists());
    assert!(!task_artifact.exists());

    let created = SecureAppPaths::prepare_task_artifact_dir(&resolver, &AlwaysSecure, task_id)
        .expect("explicit task directory");
    assert_eq!(created, task_artifact);
    assert!(created.is_dir());
}

#[test]
fn file_occupying_a_required_directory_is_rejected_without_deletion() {
    let temp = tempfile::tempdir().expect("independent test root");
    let app_root = temp.path().join(APP_DIRECTORY_NAME);
    fs::write(&app_root, b"existing data").expect("occupying file");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("syntactic resolver");

    let error =
        SecureAppPaths::prepare(&resolver, &AlwaysSecure).expect_err("file collision must fail");
    assert!(matches!(
        error,
        PlatformError::Path(ref path_error)
            if path_error.code() == PathErrorCode::PathOccupiedByFile
    ));
    assert_eq!(
        fs::read(&app_root).expect("existing file retained"),
        b"existing data"
    );
}

#[test]
fn permission_failure_returns_no_paths_and_preserves_created_directories() {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("absolute local base");
    let paths = resolver.validate_layout().expect("layout");

    let error = SecureAppPaths::prepare(&resolver, &RejectLogs)
        .expect_err("permission failure must fail closed");
    assert!(matches!(error, PlatformError::Permission(_)));
    assert!(paths.app_root.is_dir());
    assert!(paths.data_dir.is_dir());
    assert!(paths.logs_dir.is_dir());
    assert!(!paths.database_path.exists());
}

#[test]
fn symlink_or_reparse_point_in_managed_layout_is_rejected_when_environment_allows_fixture() {
    let temp = tempfile::tempdir().expect("independent test root");
    let target = temp.path().join("target");
    fs::create_dir(&target).expect("symlink target");
    let app_root = temp.path().join(APP_DIRECTORY_NAME);
    match std::os::windows::fs::symlink_dir(&target, &app_root) {
        Ok(()) => {
            let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
                .expect("syntactic resolver");
            let error = resolver
                .validate_layout()
                .expect_err("reparse point must be rejected");
            assert_eq!(error.code(), PathErrorCode::ReparsePointRejected);
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314) =>
        {
            eprintln!("symlink fixture unavailable without Windows developer-mode privilege");
        }
        Err(error) => panic!("unexpected symlink fixture error: {error}"),
    }
}

#[test]
fn only_secure_permission_status_allows_persistent_storage() {
    assert!(PermissionStatus::Secure.permits_persistent_storage());
    for status in [
        PermissionStatus::Degraded,
        PermissionStatus::Insecure,
        PermissionStatus::Unsupported,
        PermissionStatus::Unknown,
    ] {
        assert!(!status.permits_persistent_storage());
    }
}
