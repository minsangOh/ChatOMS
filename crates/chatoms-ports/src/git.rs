use std::path::{Path, PathBuf};

use chatoms_domain::{ProjectId, TaskId};

use crate::error::PortFailure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryKind {
    Git,
    NonGit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryStatus {
    pub clean: bool,
    pub detached_head: bool,
    pub current_branch: Option<String>,
    pub head_commit: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositorySafetyToken {
    pub info_attributes_digest: String,
    pub info_attributes_identity: String,
}

impl RepositoryStatus {
    #[must_use]
    pub fn ready_for_isolation(&self) -> bool {
        self.clean
            && !self.detached_head
            && self.current_branch.is_some()
            && self.head_commit.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectInspection {
    pub canonical_root: PathBuf,
    pub canonical_key: String,
    pub display_path: String,
    pub suggested_name: String,
    pub confirmation_token: String,
    pub repository_kind: RepositoryKind,
    pub repository_status: Option<RepositoryStatus>,
    pub git_common_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeCreationOutcome {
    Created,
    NoEffect,
    Uncertain,
}

pub trait GitService {
    fn is_available(&mut self) -> Result<bool, PortFailure>;

    fn inspect_project(&mut self, input: &Path) -> Result<ProjectInspection, PortFailure>;

    fn repository_status(&mut self, root: &Path) -> Result<RepositoryStatus, PortFailure>;

    fn validate_non_git_source(&mut self, root: &Path) -> Result<(), PortFailure>;

    fn validate_repository_source(
        &mut self,
        root: &Path,
        base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure>;

    fn initialize_repository(&mut self, root: &Path) -> Result<(), PortFailure>;

    fn has_commit_author(&mut self, root: &Path) -> Result<bool, PortFailure>;

    fn create_initial_snapshot(&mut self, root: &Path) -> Result<String, PortFailure>;

    fn create_task_worktree(
        &mut self,
        root: &Path,
        branch: &str,
        base_commit: &str,
        worktree: &Path,
        safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure>;

    fn verify_task_worktree(
        &mut self,
        root: &Path,
        branch: &str,
        base_commit: &str,
        worktree: &Path,
    ) -> Result<bool, PortFailure>;
}

pub trait WorktreePathProvider {
    fn prepare_worktree_path(
        &mut self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<PathBuf, PortFailure>;
}
