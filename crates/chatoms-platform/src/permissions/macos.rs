use std::path::Path;

use chatoms_ports::permissions::{
    FilesystemPermissionManager, PermissionError, PermissionErrorCode, PermissionStatus,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct MacOsPermissionManager;

impl FilesystemPermissionManager for MacOsPermissionManager {
    fn secure_directory(&self, _path: &Path) -> Result<(), PermissionError> {
        Err(PermissionError::new(
            PermissionErrorCode::UnsupportedPlatform,
        ))
    }
    fn verify_directory(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(PermissionStatus::Unsupported)
    }
    fn secure_file(&self, _path: &Path) -> Result<(), PermissionError> {
        Err(PermissionError::new(
            PermissionErrorCode::UnsupportedPlatform,
        ))
    }
    fn verify_file(&self, _path: &Path) -> Result<PermissionStatus, PermissionError> {
        Ok(PermissionStatus::Unsupported)
    }
}
