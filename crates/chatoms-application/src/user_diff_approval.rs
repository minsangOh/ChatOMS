//! Read-only user diff review and hash-bound approval for
//! `AwaitingUserDiffApproval`. Mirrors `crate::review_diff::ReviewDiffReader`
//! (same worktree/Git/filesystem identity re-verification, same bounded
//! `WorktreeDiffPort` read) but gates on `AwaitingUserDiffApproval` instead
//! of `Reviewing`, and adds a hash-bound approval step neither
//! `review_diff` nor `review_execution` need.
//!
//! This module implements the "scoped local-user-only diff exception"
//! (see `docs/DECISIONS.md`): the diff text [`UserDiffReviewReader`] reads
//! is transient, in-process data returned only so a Tauri command can hand
//! it, once, directly to the requesting local user's own review modal. It
//! is never written to SQLite, a log, or any other persistent store by this
//! module — the only thing ever persisted is the content-free
//! [`chatoms_ports::diff::DiffContentHash`] the user's approval binds to.
//! Approving does not itself start any provider, and does not transition
//! the task out of `AwaitingUserDiffApproval` — a future `Merging` Unit is
//! responsible for recomputing the current diff hash immediately before
//! merging and treating any mismatch against the approved hash as
//! fail-closed (`RecoveryRequired`).

use std::path::Path;

use chatoms_domain::{TaskId, TaskState};
use chatoms_ports::{
    TimeProvider,
    diff::{CommitCandidateOutcome, CommitCandidatePort, DiffContentHash},
    error::{FailureCategory, PortFailure},
    filesystem::FilesystemIdentityPort,
    repository::{FoundationRepository, GitIsolationStatus, RepositoryError},
};
use sha2::{Digest, Sha256};

use crate::{
    error::ApplicationError,
    tasks::{DiffApprovalView, RecordDiffApprovalRequest, TaskService},
};

pub struct ReadUserDiffForReviewRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl ReadUserDiffForReviewRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Transient diff text plus its content-free SHA-256 digest, returned
/// together so a caller never has to hash the diff text itself outside this
/// module. `Debug` deliberately hides the diff text — only its byte length
/// and digest are shown — mirroring `chatoms_ports::diff::WorktreeDiff`, so
/// a stray `{:?}` in a log statement cannot leak diff content.
#[derive(Clone, Eq, PartialEq)]
pub struct UserDiffForReview {
    diff_text: String,
    pub diff_content_hash: DiffContentHash,
}

impl UserDiffForReview {
    #[must_use]
    pub fn diff_text(&self) -> &str {
        &self.diff_text
    }
}

impl std::fmt::Debug for UserDiffForReview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UserDiffForReview")
            .field("byte_len", &self.diff_text.len())
            .field("diff_content_hash", &self.diff_content_hash)
            .finish()
    }
}

/// Computes the content-free SHA-256 digest of `diff_text`'s exact UTF-8
/// bytes — no normalization, trimming, or redaction first: the hash must
/// bind to precisely what the user was shown.
#[must_use]
pub fn hash_diff_text(diff_text: &str) -> DiffContentHash {
    let digest = Sha256::digest(diff_text.as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    DiffContentHash::from_digest_bytes(bytes)
}

pub struct UserDiffReviewReader<'a, R, F, C> {
    repository: &'a mut R,
    filesystem: &'a mut F,
    candidate: &'a mut C,
}

impl<'a, R, F, C> UserDiffReviewReader<'a, R, F, C>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
    C: CommitCandidatePort,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, filesystem: &'a mut F, candidate: &'a mut C) -> Self {
        Self {
            repository,
            filesystem,
            candidate,
        }
    }

    /// Read-only end to end, mirroring
    /// `crate::review_diff::ReviewDiffReader::read_current_diff` but gated
    /// on `AwaitingUserDiffApproval` instead of `Reviewing`: never writes to
    /// SQLite, never changes task state/version/history/lease, and never
    /// spawns a Git process if any precondition or identity check fails —
    /// an identity mismatch is rejected before
    /// [`WorktreeDiffPort::current_diff`] is ever called.
    pub fn read_current_diff(
        &mut self,
        request: &ReadUserDiffForReviewRequest,
    ) -> Result<CommitCandidateOutcome, ApplicationError> {
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::AwaitingUserDiffApproval {
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
        let (base_branch, base_commit, worktree_path) = match (
            isolation.base_branch.as_deref(),
            isolation.base_commit.as_deref(),
            isolation.worktree_path.as_deref(),
        ) {
            (Some(base_branch), Some(base_commit), Some(worktree_path)) => {
                (base_branch, base_commit, worktree_path)
            }
            _ => return Err(category_error(FailureCategory::InvariantViolation)),
        };

        let project = self
            .repository
            .get_project(task.project_id())
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;

        let root = Path::new(&project.root_path);
        let worktree = Path::new(worktree_path);
        self.filesystem
            .inspect_supported_directory(worktree)
            .and_then(|actual| self.filesystem.verify_local_tree(&actual.canonical_path))
            .map_err(port_error)?;

        self.candidate
            .current_commit_candidate(
                root,
                base_branch,
                task.task_branch_identity().as_str(),
                base_commit,
                worktree,
            )
            .map_err(port_error)
    }

    /// Reads the current diff and, on success, returns it together with its
    /// content-free hash — the shape a `get_user_diff_for_review` Tauri
    /// command needs. Every non-`Diff` outcome (`NoChanges`, `DiffTooLarge`,
    /// `TimedOut`, `Uncertain`) is rejected as a typed, content-free error;
    /// none of them ever reach the caller as if they were a usable diff.
    pub fn read_diff_for_review(
        &mut self,
        request: &ReadUserDiffForReviewRequest,
    ) -> Result<UserDiffForReview, ApplicationError> {
        match self.read_current_diff(request)? {
            CommitCandidateOutcome::Candidate(candidate) => Ok(UserDiffForReview {
                diff_text: candidate.text().to_owned(),
                diff_content_hash: candidate.content_hash(),
            }),
            CommitCandidateOutcome::NoChanges
            | CommitCandidateOutcome::CandidateTooLarge
            | CommitCandidateOutcome::TimedOut
            | CommitCandidateOutcome::Uncertain => Err(category_error(FailureCategory::Conflict)),
        }
    }
}

pub struct ApproveUserDiffRequest {
    task_id: TaskId,
    expected_version: u64,
    expected_diff_content_hash: DiffContentHash,
}

impl ApproveUserDiffRequest {
    #[must_use]
    pub const fn new(
        task_id: TaskId,
        expected_version: u64,
        expected_diff_content_hash: DiffContentHash,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            expected_diff_content_hash,
        }
    }
}

pub struct UserDiffApprovalService<'a, R, T, F, C> {
    repository: &'a mut R,
    time: &'a mut T,
    filesystem: &'a mut F,
    candidate: &'a mut C,
}

impl<'a, R, T, F, C> UserDiffApprovalService<'a, R, T, F, C>
where
    R: FoundationRepository,
    T: TimeProvider,
    F: FilesystemIdentityPort,
    C: CommitCandidatePort,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        time: &'a mut T,
        filesystem: &'a mut F,
        candidate: &'a mut C,
    ) -> Self {
        Self {
            repository,
            time,
            filesystem,
            candidate,
        }
    }

    /// Recomputes the task's current worktree diff and its content hash,
    /// and only if it exactly matches
    /// `request.expected_diff_content_hash` does it atomically
    /// create-or-reuse a `DiffApprovalRecord` for `(task_id,
    /// expected_version, diff_content_hash)`. On a hash mismatch, stale
    /// version, invalid isolation, or diff read failure, no approval row is
    /// created and the task's state/version/history/lease are left
    /// untouched. The returned error never carries the raw diff text, the
    /// mismatched hash, or any path.
    pub fn approve(
        &mut self,
        request: ApproveUserDiffRequest,
    ) -> Result<DiffApprovalView, ApplicationError> {
        let outcome = UserDiffReviewReader::new(self.repository, self.filesystem, self.candidate)
            .read_current_diff(&ReadUserDiffForReviewRequest::new(
            request.task_id,
            request.expected_version,
        ))?;
        let candidate = match outcome {
            CommitCandidateOutcome::Candidate(candidate) => candidate,
            CommitCandidateOutcome::NoChanges
            | CommitCandidateOutcome::CandidateTooLarge
            | CommitCandidateOutcome::TimedOut
            | CommitCandidateOutcome::Uncertain => {
                return Err(category_error(FailureCategory::Conflict));
            }
        };
        let current_hash = candidate.content_hash();
        if current_hash != request.expected_diff_content_hash {
            return Err(category_error(FailureCategory::Conflict));
        }

        let approved_at_ms = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        TaskService::new(self.repository, self.time).record_diff_approval(
            RecordDiffApprovalRequest::new(
                request.task_id,
                request.expected_version,
                current_hash,
                approved_at_ms,
            ),
        )
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
