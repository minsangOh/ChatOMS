use std::path::{Path, PathBuf};

use crate::error::{FailureCategory, PortFailure};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryIdentity {
    pub canonical_path: PathBuf,
    pub volume_serial_hex: String,
    pub file_id_hex: String,
}

impl DirectoryIdentity {
    #[must_use]
    pub fn same_object(&self, other: &Self) -> bool {
        self.volume_serial_hex == other.volume_serial_hex && self.file_id_hex == other.file_id_hex
    }
}

pub trait DirectoryIdentityGuard: Send {
    fn identity(&self) -> &DirectoryIdentity;
}

pub trait FilesystemIdentityPort {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure>;

    fn verify_local_tree(&mut self, root: &Path) -> Result<(), PortFailure>;

    fn acquire_guard(
        &mut self,
        path: &Path,
        expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure>;

    /// Inspects a single regular file (not a directory) and returns its
    /// stable NTFS object identity, for callers that need to bind trust to
    /// one specific file — e.g. a validation tool executable — rather than
    /// a directory tree. Implementations must reject anything that is not a
    /// regular file, including reparse points/symlinks. Defaults to
    /// fail-closed [`FailureCategory::Unsupported`] on platforms without an
    /// implementation.
    fn inspect_supported_file(&mut self, _path: &Path) -> Result<DirectoryIdentity, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}
