//! Port boundary for `git merge --abort` on the *original checkout* of a
//! task's `MergeConflict` that a user has explicitly approved to abandon.
//! This is the only Git write this port performs -- no `--quit`, `reset`,
//! `checkout`, `restore`, `stash`, or `clean`.
//!
//! Distinct from [`crate::merge_continue::MergeContinuePort`]: that port
//! *continues* a specific confirmed staged resolution and requires a
//! `Ready` manual-resolution candidate, while this port *discards* the
//! merge and its primary use case is an unresolved conflict, so it never
//! requires -- or binds an approval to -- a resolution digest.

use crate::filesystem::DirectoryIdentity;
use chatoms_domain::{ProjectId, TaskId};

/// Everything [`MergeAbortPort::abort_merge`] needs, and nothing more:
/// content-free identity, never raw diff/path/content beyond directory
/// identity. `base_commit`/`task_commit`/`merge_head_commit` are the exact
/// values an immutable `task_merge_abort_approvals` row already bound its
/// approval to -- the adapter re-verifies all of them against the live
/// repository before writing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeAbortRequest {
    pub original_checkout: DirectoryIdentity,
    pub original_common_dir: DirectoryIdentity,
    pub task_worktree: DirectoryIdentity,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub merge_conflict_task_version: u64,
    pub source_approval_task_version: u64,
    pub base_branch: String,
    pub task_branch: String,
    pub base_commit: String,
    pub task_commit: String,
    pub merge_head_commit: String,
}

/// Closed disposition of a pre-write safety check that ran before any Git
/// write was attempted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeAbortPreWriteRejection {
    /// Identity or topology of the original checkout, its common dir, or
    /// the task worktree could not be confirmed to match the request.
    IdentityOrTopology,
    /// `MERGE_AUTOSTASH` is present. Per `git-merge(1)`, `merge --abort`
    /// applies that stash entry to the worktree when present -- writing
    /// unapproved content into the original checkout -- so this is always
    /// rejected before any write is attempted.
    AutostashPresent,
    /// A rebase/cherry-pick/revert/bisect/sequencer operation is also in
    /// progress.
    ForeignOperationResidue,
    /// The repository's filter/attributes configuration could not be
    /// confirmed safe.
    UnsafeRepositoryConfiguration,
    /// A merge is in progress, but its `MERGE_HEAD` does not match the
    /// approved `task_commit`/`merge_head_commit`.
    MergeIdentityMismatch,
    /// No merge is in progress, and the repository could not be confirmed
    /// to already be fully restored to the approved base commit either.
    NotInMergeAndNotRestored,
}

/// Closed disposition of one `abort_merge` attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeAbortOutcome {
    /// `git merge --abort` exited 0 and the restoration postcondition (no
    /// merge in progress, no residue, base branch/commit restored,
    /// repository clean, task worktree/branch/commit unchanged) all held.
    Aborted,
    /// No Git write was attempted (or the write failed), but the
    /// restoration postcondition independently held anyway -- an earlier
    /// abort attempt already restored the repository and only its SQLite
    /// commit never landed. Never returned unless every restoration check
    /// independently passes.
    ConfirmedNotInMerge,
    /// A precondition (identity, topology, autostash, foreign operation
    /// residue, configuration, or merge-head identity) could not be
    /// confirmed safe before any write -- no Git write was attempted.
    PreWriteRejected(MergeAbortPreWriteRejection),
    /// Exit 0 but the postcondition did not hold, or the write's outcome
    /// (nonzero exit without full restoration, timeout, spawn failure, or
    /// any other uncertainty) could not be confirmed either way.
    PostWriteUncertain,
}

/// Performs the single write this Unit supports:
/// `git ... -C <original-checkout> merge --abort`. Implementations must
/// re-verify every precondition fresh immediately before spawning -- never
/// trust a caller's earlier read alone.
pub trait MergeAbortPort {
    fn abort_merge(&mut self, request: &MergeAbortRequest) -> MergeAbortOutcome;
}
