#![doc = "Platform boundary for Windows-first implementations and compile-time macOS ports."]
#![deny(unsafe_op_in_unsafe_fn)]

use std::{fs, path::Path};

use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    path::{AppPathResolver, PathError, PathErrorCode, ResolvedAppPaths, TaskId},
    permissions::{FilesystemPermissionManager, PermissionError, PermissionStatus},
};
use thiserror::Error;

pub mod bootstrap;
pub mod path;
pub mod permissions;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("application path preparation failed")]
    Path(#[from] PathError),
    #[error("filesystem permission preparation failed")]
    Permission(#[from] PermissionError),
    #[error("filesystem permission verification did not reach a secure state")]
    PermissionNotSecure(PermissionStatus),
}

impl CategorizedFailure for PlatformError {
    fn category(&self) -> FailureCategory {
        match self {
            Self::Path(error) => error.category(),
            Self::Permission(error) => error.category(),
            Self::PermissionNotSecure(_) => FailureCategory::StorageInsecure,
        }
    }
}

pub struct SecureAppPaths;

impl SecureAppPaths {
    pub fn prepare(
        resolver: &impl AppPathResolver,
        permissions: &impl FilesystemPermissionManager,
    ) -> Result<ResolvedAppPaths, PlatformError> {
        let paths = resolver.validate_layout()?;
        for directory in [
            &paths.app_root,
            &paths.data_dir,
            &paths.logs_dir,
            &paths.artifacts_dir,
            &paths.temp_dir,
        ] {
            ensure_directory(directory)?;
            resolver.validate_managed_path(directory)?;
            secure_and_verify_directory(permissions, directory)?;
        }
        Ok(resolver.validate_layout()?)
    }

    pub fn prepare_task_artifact_dir(
        resolver: &impl AppPathResolver,
        permissions: &impl FilesystemPermissionManager,
        task_id: TaskId,
    ) -> Result<std::path::PathBuf, PlatformError> {
        let path = resolver.task_artifact_dir(task_id)?;
        prepare_task_directory(resolver, permissions, &path)?;
        Ok(path)
    }

    pub fn prepare_task_temp_dir(
        resolver: &impl AppPathResolver,
        permissions: &impl FilesystemPermissionManager,
        task_id: TaskId,
    ) -> Result<std::path::PathBuf, PlatformError> {
        let path = resolver.task_temp_dir(task_id)?;
        prepare_task_directory(resolver, permissions, &path)?;
        Ok(path)
    }
}

fn prepare_task_directory(
    resolver: &impl AppPathResolver,
    permissions: &impl FilesystemPermissionManager,
    path: &Path,
) -> Result<(), PlatformError> {
    resolver.validate_layout()?;
    resolver.validate_managed_path(path)?;
    ensure_directory(path)?;
    resolver.validate_managed_path(path)?;
    secure_and_verify_directory(permissions, path)
}

fn ensure_directory(path: &Path) -> Result<(), PathError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(PathError::new(PathErrorCode::PathOccupiedByFile)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .map_err(|source| PathError::with_source(PathErrorCode::CreateDirectoryFailed, source)),
        Err(source) => Err(PathError::with_source(
            PathErrorCode::CreateDirectoryFailed,
            source,
        )),
    }
}

fn secure_and_verify_directory(
    permissions: &impl FilesystemPermissionManager,
    path: &Path,
) -> Result<(), PlatformError> {
    permissions.secure_directory(path)?;
    let status = permissions.verify_directory(path)?;
    if status.permits_persistent_storage() {
        Ok(())
    } else {
        Err(PlatformError::PermissionNotSecure(status))
    }
}
