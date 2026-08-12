#![doc = "Platform boundary for Windows-first implementations and compile-time macOS ports."]
#![deny(unsafe_op_in_unsafe_fn)]

use std::{fs, path::Path};

use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    filesystem::{DirectoryIdentity, FilesystemIdentityPort},
    git::WorktreePathProvider,
    path::{AppPathResolver, PathError, PathErrorCode, ResolvedAppPaths, TaskId},
    permissions::{FilesystemPermissionManager, PermissionError, PermissionStatus},
};
use thiserror::Error;

pub mod bootstrap;
#[cfg(windows)]
pub mod claude_trust;
pub mod filesystem;
#[cfg(windows)]
pub mod git_runtime;
pub mod path;
pub mod permissions;
#[cfg(windows)]
pub mod preflight;

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

/// Creates one application-owned path component only after its nearest existing
/// ancestor and the created final path have passed the Windows storage gate.
/// Callers must invoke this one component at a time; it intentionally never
/// uses recursive directory creation.
pub fn ensure_supported_directory(path: &Path) -> Result<(), PlatformError> {
    ensure_directory(path)?;
    Ok(())
}

/// Returns the handle-derived identity used to revalidate an existing
/// application-owned directory before a security-sensitive child process.
pub fn supported_directory_identity(path: &Path) -> Result<DirectoryIdentity, PlatformError> {
    #[cfg(windows)]
    {
        let mut filesystem = crate::filesystem::WindowsFilesystemIdentity;
        filesystem
            .inspect_supported_directory(path)
            .map_err(|_| PlatformError::Path(PathError::new(PathErrorCode::InvalidBasePath)))
    }
    #[cfg(not(windows))]
    {
        let canonical_path = fs::canonicalize(path).map_err(|source| {
            PlatformError::Path(PathError::with_source(
                PathErrorCode::InvalidBasePath,
                source,
            ))
        })?;
        Ok(DirectoryIdentity {
            canonical_path,
            volume_serial_hex: "unsupported-platform".to_owned(),
            file_id_hex: "unsupported-platform".to_owned(),
        })
    }
}

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
            &paths.worktrees_dir,
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

    /// Prepares the fixed, system-level Claude/Codex provider preflight
    /// working directory. This directory is never the project root, a task
    /// worktree, or the inherited process current directory, and it is never
    /// combined with a profile or task identity.
    pub fn prepare_provider_preflight_dir(
        resolver: &impl AppPathResolver,
        permissions: &impl FilesystemPermissionManager,
    ) -> Result<std::path::PathBuf, PlatformError> {
        let path = resolver.provider_preflight_dir()?;
        prepare_task_directory(resolver, permissions, &path)?;
        Ok(path)
    }
}

pub struct ManagedWorktreePaths<R, P> {
    resolver: R,
    permissions: P,
}

impl<R, P> ManagedWorktreePaths<R, P> {
    #[must_use]
    pub const fn new(resolver: R, permissions: P) -> Self {
        Self {
            resolver,
            permissions,
        }
    }
}

#[cfg(windows)]
impl
    ManagedWorktreePaths<
        crate::path::WindowsPathResolver,
        crate::permissions::WindowsPermissionManager,
    >
{
    pub fn windows_from_environment() -> Result<Self, chatoms_ports::error::PortFailure> {
        let resolver = crate::path::WindowsPathResolver::from_environment()
            .map_err(|error| port_failure(&error))?;
        Ok(Self::new(
            resolver,
            crate::permissions::WindowsPermissionManager,
        ))
    }
}

impl<R, P> WorktreePathProvider for ManagedWorktreePaths<R, P>
where
    R: AppPathResolver,
    P: FilesystemPermissionManager,
{
    fn prepare_worktree_path(
        &mut self,
        project_id: chatoms_ports::path::ProjectId,
        task_id: TaskId,
    ) -> Result<std::path::PathBuf, chatoms_ports::error::PortFailure> {
        let paths = self
            .resolver
            .validate_layout()
            .map_err(|error| port_failure(&error))?;
        let target = self
            .resolver
            .task_worktree_dir(project_id, task_id)
            .map_err(|error| port_failure(&error))?;
        let project_parent = target
            .parent()
            .ok_or_else(|| chatoms_ports::error::PortFailure::new(FailureCategory::InvalidInput))?;
        if project_parent.parent() != Some(paths.worktrees_dir.as_path()) {
            return Err(chatoms_ports::error::PortFailure::new(
                FailureCategory::InvalidInput,
            ));
        }
        prepare_task_directory(&self.resolver, &self.permissions, project_parent)
            .map_err(|error| port_failure(&error))?;
        self.resolver
            .validate_managed_path(&target)
            .map_err(|error| port_failure(&error))?;
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(chatoms_ports::error::PortFailure::new(
                FailureCategory::AlreadyExists,
            ));
        }
        Ok(target)
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
    validate_creation_ancestor(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            validate_created_directory(path)?;
            Ok(())
        }
        Ok(_) => Err(PathError::new(PathErrorCode::PathOccupiedByFile)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|source| {
                PathError::with_source(PathErrorCode::CreateDirectoryFailed, source)
            })?;
            validate_created_directory(path)
        }
        Err(source) => Err(PathError::with_source(
            PathErrorCode::CreateDirectoryFailed,
            source,
        )),
    }
}

fn validate_creation_ancestor(path: &Path) -> Result<(), PathError> {
    #[cfg(windows)]
    {
        let ancestor = path
            .ancestors()
            .find(|candidate| candidate.exists())
            .ok_or_else(|| PathError::new(PathErrorCode::InvalidBasePath))?;
        let mut filesystem = crate::filesystem::WindowsFilesystemIdentity;
        filesystem
            .inspect_supported_directory(ancestor)
            .map_err(|_| PathError::new(PathErrorCode::InvalidBasePath))?;
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
}

fn validate_created_directory(path: &Path) -> Result<(), PathError> {
    #[cfg(windows)]
    {
        let mut filesystem = crate::filesystem::WindowsFilesystemIdentity;
        filesystem
            .inspect_supported_directory(path)
            .map_err(|_| PathError::new(PathErrorCode::InvalidBasePath))?;
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
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

fn port_failure(error: &impl CategorizedFailure) -> chatoms_ports::error::PortFailure {
    chatoms_ports::error::PortFailure::with_policy(
        error.category(),
        error.severity(),
        error.retry(),
    )
}
