use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, UNIX_EPOCH},
};

use chatoms_platform::bootstrap::{
    StaticPlatformCapabilityAdapter, StorageBootstrapAdapter, SystemTimeProvider,
    system_time_to_unix_epoch_ms,
};
use chatoms_ports::{
    PlatformCapabilityPort, PlatformCapabilityStatus, StorageBootstrapPort, StorageBootstrapState,
    TimeProvider,
    error::{CategorizedFailure, FailureCategory},
    filesystem::FilesystemIdentityPort,
    path::{AppPathResolver, PathError, PathErrorCode, ProjectId, ResolvedAppPaths, TaskId},
    permissions::{FilesystemPermissionManager, PermissionError, PermissionStatus},
};
use tempfile::TempDir;

#[derive(Clone)]
struct TempResolver {
    paths: ResolvedAppPaths,
    reject_layout: bool,
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("run mklink junction fixture");
    assert!(
        output.status.success(),
        "junction fixture is mandatory: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn real_junction_alias_has_same_identity_and_rebinding_is_detected() {
    let temp = TempDir::new().expect("temp");
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let alias = temp.path().join("alias");
    std::fs::create_dir(&first).expect("first target");
    std::fs::create_dir(&second).expect("second target");
    create_junction(&alias, &first);

    let mut filesystem = chatoms_platform::filesystem::WindowsFilesystemIdentity;
    let direct = filesystem
        .inspect_supported_directory(&first)
        .expect("direct identity");
    let through_alias = filesystem
        .inspect_supported_directory(&alias)
        .expect("junction alias identity");
    assert!(direct.same_object(&through_alias));

    std::fs::remove_dir(&alias).expect("remove test junction only");
    assert!(
        first.is_dir(),
        "junction removal must not remove its target"
    );
    create_junction(&alias, &second);
    let rebound = filesystem
        .inspect_supported_directory(&alias)
        .expect("rebound alias identity");
    assert!(!direct.same_object(&rebound));
    assert!(filesystem.acquire_guard(&alias, &direct).is_err());
}

impl TempResolver {
    fn new(temp: &TempDir) -> Self {
        let app_root = temp.path().join("ChatOMS");
        Self {
            paths: ResolvedAppPaths {
                data_dir: app_root.join("data"),
                database_path: app_root.join("data/chatoms.sqlite3"),
                logs_dir: app_root.join("logs"),
                artifacts_dir: app_root.join("artifacts"),
                temp_dir: app_root.join("temp"),
                worktrees_dir: app_root.join("worktrees"),
                app_root,
            },
            reject_layout: false,
        }
    }
}

impl AppPathResolver for TempResolver {
    fn app_data_dir(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.app_root.clone())
    }
    fn database_path(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.database_path.clone())
    }
    fn logs_dir(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.logs_dir.clone())
    }
    fn artifacts_dir(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.artifacts_dir.clone())
    }
    fn temp_dir(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.temp_dir.clone())
    }
    fn worktrees_dir(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.worktrees_dir.clone())
    }
    fn task_artifact_dir(&self, task_id: TaskId) -> Result<PathBuf, PathError> {
        Ok(self.paths.artifacts_dir.join(task_id.to_string()))
    }
    fn task_temp_dir(&self, task_id: TaskId) -> Result<PathBuf, PathError> {
        Ok(self.paths.temp_dir.join(task_id.to_string()))
    }
    fn task_worktree_dir(
        &self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<PathBuf, PathError> {
        Ok(self
            .paths
            .worktrees_dir
            .join(project_id.to_string())
            .join(task_id.to_string()))
    }
    fn provider_preflight_dir(&self) -> Result<PathBuf, PathError> {
        Ok(self.paths.temp_dir.join("provider-preflight"))
    }
    fn validate_layout(&self) -> Result<ResolvedAppPaths, PathError> {
        if self.reject_layout {
            Err(PathError::new(PathErrorCode::ReparsePointRejected))
        } else {
            Ok(self.paths.clone())
        }
    }
    fn validate_managed_path(&self, _path: &Path) -> Result<(), PathError> {
        if self.reject_layout {
            Err(PathError::new(PathErrorCode::ReparsePointRejected))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
struct PermissionFake {
    status: PermissionStatus,
}

impl FilesystemPermissionManager for PermissionFake {
    fn secure_directory(&self, _path: &Path) -> Result<(), PermissionError> {
        if self.status == PermissionStatus::Secure {
            Ok(())
        } else {
            Err(PermissionError::new(
                chatoms_ports::permissions::PermissionErrorCode::PermissionDenied,
            ))
        }
    }
    fn verify_directory(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(self.status)
    }
    fn secure_file(&self, _path: &Path) -> Result<(), PermissionError> {
        Ok(())
    }
    fn verify_file(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(self.status)
    }
}

#[test]
fn tempfile_storage_prepare_succeeds_without_local_app_data() {
    let temp = TempDir::new().expect("temp");
    let resolver = TempResolver::new(&temp);
    let expected = resolver.paths.clone();
    let shared = Arc::new(Mutex::new(None));
    let mut adapter = StorageBootstrapAdapter::new(
        resolver,
        PermissionFake {
            status: PermissionStatus::Secure,
        },
        shared.clone(),
    );
    assert_eq!(
        adapter.prepare_secure_storage().expect("storage"),
        StorageBootstrapState::Ready
    );
    assert_eq!(*shared.lock().expect("paths"), Some(expected));
}

#[test]
fn permission_and_reparse_failures_are_categorized_and_store_no_paths() {
    let temp = TempDir::new().expect("temp");
    let shared = Arc::new(Mutex::new(None));
    let mut denied = StorageBootstrapAdapter::new(
        TempResolver::new(&temp),
        PermissionFake {
            status: PermissionStatus::Insecure,
        },
        shared.clone(),
    );
    assert_eq!(
        denied
            .prepare_secure_storage()
            .expect_err("permission failure")
            .category(),
        FailureCategory::PermissionDenied
    );

    let mut resolver = TempResolver::new(&temp);
    resolver.reject_layout = true;
    let mut reparse = StorageBootstrapAdapter::new(
        resolver,
        PermissionFake {
            status: PermissionStatus::Secure,
        },
        shared.clone(),
    );
    assert_eq!(
        reparse
            .prepare_secure_storage()
            .expect_err("reparse rejection")
            .category(),
        FailureCategory::StorageInsecure
    );
    assert!(shared.lock().expect("paths").is_none());
}

#[test]
fn system_time_and_static_capabilities_are_safe_and_bounded() {
    let mut time = SystemTimeProvider;
    assert!(time.now_ms().expect("current time") >= 0);
    assert_eq!(
        system_time_to_unix_epoch_ms(UNIX_EPOCH + Duration::from_millis(123)).expect("time"),
        123
    );
    assert_eq!(
        system_time_to_unix_epoch_ms(UNIX_EPOCH - Duration::from_millis(1))
            .expect_err("pre-epoch")
            .category(),
        FailureCategory::Internal
    );
    let mut capabilities = StaticPlatformCapabilityAdapter;
    let status = capabilities.platform_capabilities().expect("capabilities");
    assert_eq!(status.secure_storage, PlatformCapabilityStatus::Supported);
    assert_eq!(
        status.native_permissions,
        PlatformCapabilityStatus::Supported
    );
}
