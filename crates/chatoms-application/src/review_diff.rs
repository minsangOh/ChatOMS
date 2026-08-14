//! Read-only use case for fetching a task's current worktree Git diff — the
//! data a future Claude Review adapter will pass as ephemeral stdin (see
//! `docs/SECURITY_POLICY.md`). Mirrors the read-only-precondition shape of
//! `crate::testing_execution::TestingBatchStarter` (no state transition to
//! drive), but adds a step neither that nor
//! `crate::planning_execution`/`crate::implementation_execution` need:
//! because this use case itself spawns a trusted Git process against the
//! stored worktree path — rather than just handing that path string to a
//! provider CLI to read at its own risk — it re-verifies the worktree's Git
//! identity via [`GitService::verify_task_worktree`] and its filesystem
//! identity via [`FilesystemIdentityPort`] immediately before every read,
//! never trusting the stored `WorktreeReady` isolation record on its own.
//!
//! The returned [`WorktreeDiffOutcome`] must never be persisted to SQLite,
//! placed on a DTO/IPC surface, or logged — it exists only for a caller to
//! hand, unmodified, to a future Claude Review adapter's stdin.

use std::path::Path;

use chatoms_domain::{TaskId, TaskState};
use chatoms_ports::{
    diff::{WorktreeDiffOutcome, WorktreeDiffPort},
    error::{FailureCategory, PortFailure},
    filesystem::FilesystemIdentityPort,
    git::GitService,
    repository::{FoundationRepository, GitIsolationStatus, RepositoryError},
};

use crate::error::ApplicationError;

pub struct ReadWorktreeDiffRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl ReadWorktreeDiffRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

pub struct ReviewDiffReader<'a, R, G, F, D> {
    repository: &'a mut R,
    git: &'a mut G,
    filesystem: &'a mut F,
    diff: &'a mut D,
}

impl<'a, R, G, F, D> ReviewDiffReader<'a, R, G, F, D>
where
    R: FoundationRepository,
    G: GitService,
    F: FilesystemIdentityPort,
    D: WorktreeDiffPort,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        git: &'a mut G,
        filesystem: &'a mut F,
        diff: &'a mut D,
    ) -> Self {
        Self {
            repository,
            git,
            filesystem,
            diff,
        }
    }

    /// Read-only end to end: verifies the task is `Reviewing` at
    /// `expected_version`, resolves its `WorktreeReady` isolation record and
    /// owning project's root path, re-verifies the worktree's Git identity
    /// (task branch, base commit, common-dir) and filesystem identity
    /// fresh, and only then reads its current diff. Never spawns a Git
    /// process if any precondition or identity check fails — an identity
    /// mismatch is rejected before [`WorktreeDiffPort::current_diff`] is
    /// ever called.
    pub fn read_current_diff(
        &mut self,
        request: &ReadWorktreeDiffRequest,
    ) -> Result<WorktreeDiffOutcome, ApplicationError> {
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::Reviewing {
            return Err(category_error(FailureCategory::InvalidState));
        }

        let isolation = self
            .repository
            .get_task_isolation(request.task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let (base_commit, worktree_path) = match (
            isolation.base_commit.as_deref(),
            isolation.worktree_path.as_deref(),
        ) {
            (Some(base_commit), Some(worktree_path)) => (base_commit, worktree_path),
            _ => return Err(category_error(FailureCategory::InvariantViolation)),
        };

        let project = self
            .repository
            .get_project(task.project_id())
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;

        let root = Path::new(&project.root_path);
        let worktree = Path::new(worktree_path);
        let verified = self
            .git
            .verify_task_worktree(
                root,
                task.task_branch_identity().as_str(),
                base_commit,
                worktree,
            )
            .map_err(port_error)?;
        if !verified {
            return Err(category_error(FailureCategory::Conflict));
        }
        self.filesystem
            .inspect_supported_directory(worktree)
            .and_then(|actual| self.filesystem.verify_local_tree(&actual.canonical_path))
            .map_err(port_error)?;

        self.diff.current_diff(worktree).map_err(port_error)
    }
}

fn port_error(error: PortFailure) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn repository_error(error: RepositoryError) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
