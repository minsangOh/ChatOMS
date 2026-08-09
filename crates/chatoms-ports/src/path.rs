use std::{error::Error, fmt, path::PathBuf};

pub use chatoms_domain::{ProjectId, TaskId};

pub const APP_IDENTIFIER: &str = "io.github.minsangoh.chatoms";
pub const APP_DIRECTORY_NAME: &str = "ChatOMS";
pub const DATABASE_FILE_NAME: &str = "chatoms.sqlite3";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathErrorCode {
    EnvironmentUnavailable,
    InvalidBasePath,
    RelativeBasePath,
    PathOutsideRoot,
    InvalidTaskPath,
    PathOccupiedByFile,
    ReparsePointRejected,
    CreateDirectoryFailed,
}

#[derive(Debug)]
pub struct PathError {
    code: PathErrorCode,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl PathError {
    #[must_use]
    pub const fn new(code: PathErrorCode) -> Self {
        Self { code, source: None }
    }

    #[must_use]
    pub fn with_source(code: PathErrorCode, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }

    #[must_use]
    pub const fn code(&self) -> PathErrorCode {
        self.code
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "application path operation failed: {:?}",
            self.code
        )
    }
}

impl Error for PathError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAppPaths {
    pub app_root: PathBuf,
    pub data_dir: PathBuf,
    pub database_path: PathBuf,
    pub logs_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub worktrees_dir: PathBuf,
}

pub trait AppPathResolver {
    fn app_data_dir(&self) -> Result<PathBuf, PathError>;
    fn database_path(&self) -> Result<PathBuf, PathError>;
    fn logs_dir(&self) -> Result<PathBuf, PathError>;
    fn artifacts_dir(&self) -> Result<PathBuf, PathError>;
    fn temp_dir(&self) -> Result<PathBuf, PathError>;
    fn worktrees_dir(&self) -> Result<PathBuf, PathError>;
    fn task_artifact_dir(&self, task_id: TaskId) -> Result<PathBuf, PathError>;
    fn task_temp_dir(&self, task_id: TaskId) -> Result<PathBuf, PathError>;
    fn task_worktree_dir(
        &self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<PathBuf, PathError>;
    fn validate_layout(&self) -> Result<ResolvedAppPaths, PathError>;
    fn validate_managed_path(&self, path: &std::path::Path) -> Result<(), PathError>;
}
