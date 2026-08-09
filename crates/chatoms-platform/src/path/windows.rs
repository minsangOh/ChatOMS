use std::{
    env,
    ffi::OsStr,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf, Prefix},
};

use chatoms_ports::path::{
    APP_DIRECTORY_NAME, AppPathResolver, DATABASE_FILE_NAME, PathError, PathErrorCode, ProjectId,
    ResolvedAppPaths, TaskId,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

#[derive(Clone, Debug)]
pub struct WindowsPathResolver {
    base_dir: PathBuf,
    paths: ResolvedAppPaths,
}

impl WindowsPathResolver {
    pub fn from_environment() -> Result<Self, PathError> {
        let base = env::var_os("LOCALAPPDATA")
            .ok_or_else(|| PathError::new(PathErrorCode::EnvironmentUnavailable))?;
        if base.is_empty() {
            return Err(PathError::new(PathErrorCode::EnvironmentUnavailable));
        }
        Self::from_base_dir(PathBuf::from(base))
    }

    #[doc(hidden)]
    pub fn from_base_dir_for_test(base_dir: PathBuf) -> Result<Self, PathError> {
        Self::from_base_dir(base_dir)
    }

    fn from_base_dir(base_dir: PathBuf) -> Result<Self, PathError> {
        validate_base_path(&base_dir)?;
        let app_root = base_dir.join(APP_DIRECTORY_NAME);
        let data_dir = app_root.join("data");
        let paths = ResolvedAppPaths {
            database_path: data_dir.join(DATABASE_FILE_NAME),
            data_dir,
            logs_dir: app_root.join("logs"),
            artifacts_dir: app_root.join("artifacts"),
            temp_dir: app_root.join("temp"),
            worktrees_dir: app_root.join("worktrees"),
            app_root,
        };
        validate_syntactic_layout(&paths)?;
        Ok(Self { base_dir, paths })
    }

    fn validate_existing_layout(&self) -> Result<(), PathError> {
        validate_existing_directory(&self.base_dir)?;
        for directory in [
            &self.paths.app_root,
            &self.paths.data_dir,
            &self.paths.logs_dir,
            &self.paths.artifacts_dir,
            &self.paths.temp_dir,
            &self.paths.worktrees_dir,
        ] {
            validate_optional_directory(directory)?;
        }
        validate_optional_file(&self.paths.database_path)
    }
}

impl AppPathResolver for WindowsPathResolver {
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
        task_child(&self.paths.artifacts_dir, task_id)
    }

    fn task_temp_dir(&self, task_id: TaskId) -> Result<PathBuf, PathError> {
        task_child(&self.paths.temp_dir, task_id)
    }

    fn task_worktree_dir(
        &self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<PathBuf, PathError> {
        let project = identifier_component(project_id.to_string())?;
        let task = identifier_component(task_id.to_string())?;
        let project_dir = self.paths.worktrees_dir.join(project);
        let child = project_dir.join(task);
        if child.parent() != Some(project_dir.as_path())
            || !child.starts_with(&self.paths.worktrees_dir)
        {
            return Err(PathError::new(PathErrorCode::InvalidTaskPath));
        }
        Ok(child)
    }

    fn validate_layout(&self) -> Result<ResolvedAppPaths, PathError> {
        validate_syntactic_layout(&self.paths)?;
        self.validate_existing_layout()?;
        Ok(self.paths.clone())
    }

    fn validate_managed_path(&self, path: &Path) -> Result<(), PathError> {
        if !path.is_absolute() || !path.starts_with(&self.paths.app_root) {
            return Err(PathError::new(PathErrorCode::PathOutsideRoot));
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return Err(PathError::new(PathErrorCode::InvalidTaskPath));
        }
        reject_nul(path.as_os_str())?;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if is_reparse_point(&metadata) => {
                Err(PathError::new(PathErrorCode::ReparsePointRejected))
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PathError::with_source(
                PathErrorCode::InvalidBasePath,
                source,
            )),
        }
    }
}

fn validate_base_path(path: &Path) -> Result<(), PathError> {
    if path.as_os_str().is_empty() {
        return Err(PathError::new(PathErrorCode::InvalidBasePath));
    }
    reject_nul(path.as_os_str())?;
    if !path.is_absolute() {
        return Err(PathError::new(PathErrorCode::RelativeBasePath));
    }
    match path.components().next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => Ok(()),
        _ => Err(PathError::new(PathErrorCode::InvalidBasePath)),
    }
}

fn reject_nul(value: &OsStr) -> Result<(), PathError> {
    if value.encode_wide().any(|unit| unit == 0) {
        Err(PathError::new(PathErrorCode::InvalidBasePath))
    } else {
        Ok(())
    }
}

fn validate_syntactic_layout(paths: &ResolvedAppPaths) -> Result<(), PathError> {
    if !paths.app_root.is_absolute()
        || !paths.data_dir.starts_with(&paths.app_root)
        || !paths.logs_dir.starts_with(&paths.app_root)
        || !paths.artifacts_dir.starts_with(&paths.app_root)
        || !paths.temp_dir.starts_with(&paths.app_root)
        || !paths.worktrees_dir.starts_with(&paths.app_root)
        || !paths.database_path.starts_with(&paths.data_dir)
        || paths.database_path.file_name() != Some(OsStr::new(DATABASE_FILE_NAME))
    {
        return Err(PathError::new(PathErrorCode::PathOutsideRoot));
    }
    Ok(())
}

fn task_child(parent: &Path, task_id: TaskId) -> Result<PathBuf, PathError> {
    let component = identifier_component(task_id.to_string())?;
    let child = parent.join(component);
    if child.parent() != Some(parent) || !child.starts_with(parent) {
        return Err(PathError::new(PathErrorCode::InvalidTaskPath));
    }
    Ok(child)
}

fn identifier_component(component: String) -> Result<String, PathError> {
    if component.len() != 36
        || component
            .bytes()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'-')
    {
        return Err(PathError::new(PathErrorCode::InvalidTaskPath));
    }
    Ok(component)
}

fn validate_existing_directory(path: &Path) -> Result<(), PathError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse_point(&metadata) => {
            Err(PathError::new(PathErrorCode::ReparsePointRejected))
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(PathError::new(PathErrorCode::PathOccupiedByFile)),
        Err(source) => Err(PathError::with_source(
            PathErrorCode::InvalidBasePath,
            source,
        )),
    }
}

fn validate_optional_directory(path: &Path) -> Result<(), PathError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse_point(&metadata) => {
            Err(PathError::new(PathErrorCode::ReparsePointRejected))
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(PathError::new(PathErrorCode::PathOccupiedByFile)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PathError::with_source(
            PathErrorCode::InvalidBasePath,
            source,
        )),
    }
}

fn validate_optional_file(path: &Path) -> Result<(), PathError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse_point(&metadata) => {
            Err(PathError::new(PathErrorCode::ReparsePointRejected))
        }
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(PathError::new(PathErrorCode::PathOccupiedByFile)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PathError::with_source(
            PathErrorCode::InvalidBasePath,
            source,
        )),
    }
}

fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}
