//! Trusted working directory for Claude/Codex provider preflight.
//!
//! This directory is a system-level boundary: it is never the project root,
//! a task worktree, or the inherited process current directory, and it is
//! never combined with a profile or task identity. [`Self::prepare`] runs
//! once to create, secure, and capture the directory's identity;
//! [`TrustedPreflightWorkingDirectory::revalidate`] must run again
//! immediately before every process spawn that uses this directory as its
//! working directory.

use std::path::{Path, PathBuf};

use chatoms_ports::{
    filesystem::DirectoryIdentity,
    path::{AppPathResolver, PathError, PathErrorCode},
    permissions::FilesystemPermissionManager,
};

use crate::{PlatformError, SecureAppPaths, supported_directory_identity};

/// A provider preflight working directory whose path and handle-derived
/// identity have been verified. Holding one is proof the checks passed at
/// [`Self::prepare`] time; call [`Self::revalidate`] again immediately
/// before every use.
#[derive(Clone, Debug)]
pub struct TrustedPreflightWorkingDirectory {
    path: PathBuf,
    identity: DirectoryIdentity,
}

impl TrustedPreflightWorkingDirectory {
    /// Creates (if absent), secures, and captures the identity of the
    /// fixed provider preflight directory under the app-owned `temp`
    /// subdirectory. Any creation, ACL, or identity failure is fail-closed.
    pub fn prepare(
        resolver: &impl AppPathResolver,
        permissions: &impl FilesystemPermissionManager,
    ) -> Result<Self, PlatformError> {
        let path = SecureAppPaths::prepare_provider_preflight_dir(resolver, permissions)?;
        let identity = supported_directory_identity(&path)?;
        Ok(Self { path, identity })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-inspects the directory at this trusted path and confirms it is
    /// still non-reparse, secure, and the same object (volume + file ID and
    /// canonical path) captured at [`Self::prepare`] time. Callers must
    /// invoke this immediately before every execution that uses [`path`];
    /// the directory can be replaced or removed after the initial check.
    ///
    /// [`path`]: Self::path
    pub fn revalidate(&self) -> Result<(), PlatformError> {
        let current = supported_directory_identity(&self.path)?;
        if !current.same_object(&self.identity)
            || current.canonical_path != self.identity.canonical_path
        {
            return Err(identity_mismatch());
        }
        Ok(())
    }
}

fn identity_mismatch() -> PlatformError {
    PlatformError::Path(PathError::new(PathErrorCode::InvalidBasePath))
}
