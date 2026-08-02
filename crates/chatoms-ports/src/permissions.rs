use std::{error::Error, fmt, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionStatus {
    Secure,
    Degraded,
    Insecure,
    Unsupported,
    Unknown,
}

impl PermissionStatus {
    #[must_use]
    pub const fn permits_persistent_storage(self) -> bool {
        matches!(self, Self::Secure)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionErrorCode {
    CurrentUserSidUnavailable,
    ReadAclFailed,
    WriteAclFailed,
    VerifyAclFailed,
    InsecureAcl,
    UnsupportedPlatform,
    PermissionDenied,
    InvariantViolation,
}

#[derive(Debug)]
pub struct PermissionError {
    code: PermissionErrorCode,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl PermissionError {
    #[must_use]
    pub const fn new(code: PermissionErrorCode) -> Self {
        Self { code, source: None }
    }

    #[must_use]
    pub fn with_source(
        code: PermissionErrorCode,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }

    #[must_use]
    pub const fn code(&self) -> PermissionErrorCode {
        self.code
    }
}

impl fmt::Display for PermissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "filesystem permission operation failed: {:?}",
            self.code
        )
    }
}

impl Error for PermissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

pub trait FilesystemPermissionManager {
    fn secure_directory(&self, path: &Path) -> Result<(), PermissionError>;
    fn verify_directory(&self, path: &Path) -> Result<PermissionStatus, PermissionError>;
    fn secure_file(&self, path: &Path) -> Result<(), PermissionError>;
    fn verify_file(&self, path: &Path) -> Result<PermissionStatus, PermissionError>;

    fn describe_status(&self, path: &Path) -> Result<PermissionStatus, PermissionError> {
        if path.is_dir() {
            self.verify_directory(path)
        } else {
            self.verify_file(path)
        }
    }
}
