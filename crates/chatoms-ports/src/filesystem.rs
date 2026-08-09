use std::path::{Path, PathBuf};

use crate::error::PortFailure;

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
}
