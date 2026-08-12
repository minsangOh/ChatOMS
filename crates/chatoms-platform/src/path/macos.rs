use std::path::{Path, PathBuf};

use chatoms_ports::path::{
    AppPathResolver, PathError, PathErrorCode, ProjectId, ResolvedAppPaths, TaskId,
};

#[derive(Clone, Debug, Default)]
pub struct MacOsPathResolver;

impl AppPathResolver for MacOsPathResolver {
    fn app_data_dir(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn database_path(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn logs_dir(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn artifacts_dir(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn temp_dir(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn worktrees_dir(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn task_artifact_dir(&self, _task_id: TaskId) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn task_temp_dir(&self, _task_id: TaskId) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn task_worktree_dir(
        &self,
        _project_id: ProjectId,
        _task_id: TaskId,
    ) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn provider_preflight_dir(&self) -> Result<PathBuf, PathError> {
        unsupported()
    }
    fn validate_layout(&self) -> Result<ResolvedAppPaths, PathError> {
        unsupported()
    }
    fn validate_managed_path(&self, _path: &Path) -> Result<(), PathError> {
        unsupported()
    }
}

fn unsupported<T>() -> Result<T, PathError> {
    Err(PathError::new(PathErrorCode::EnvironmentUnavailable))
}
