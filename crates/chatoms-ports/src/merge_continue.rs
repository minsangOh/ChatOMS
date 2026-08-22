//! Port boundary for `git merge --continue` on the *original checkout* of a
//! task whose `MergeConflict` has an immutable, confirmed manual resolution
//! (see [`crate::manual_merge_resolution`]). This is the only Git write this
//! port performs — no abort, reset, or automatic conflict resolution.

use crate::{filesystem::DirectoryIdentity, manual_merge_resolution::ManualResolutionDigest};
use chatoms_domain::{ProjectId, TaskId};

/// Everything [`MergeContinuePort::continue_merge`] needs, and nothing more:
/// content-free identity, never raw diff/path/content. `confirmed_*` fields
/// are the exact values an immutable
/// `task_manual_merge_resolution_confirmations` row already bound its
/// digest to — the adapter re-verifies all of them against the live
/// repository before writing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeContinueRequest {
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
    pub confirmed_resolution_digest: ManualResolutionDigest,
}

/// Closed disposition of one `continue_merge` attempt. Cancellation is not
/// part of this vocabulary — a short `merge --continue` commit write is
/// never interrupted mid-flight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeContinueOutcome {
    /// Exit 0, and the postcondition (`HEAD` has exactly the two expected
    /// parents, base branch retained, `MERGE_HEAD`/residue gone, repository
    /// clean, post-commit staged-index digest still matches) all held.
    Continued,
    /// The live staged-index digest no longer matches
    /// `confirmed_resolution_digest`, but the repository is confirmed to
    /// still be in an ordinary merge-in-progress topology — the staged
    /// result changed after confirmation.
    ConfirmationStale,
    /// The repository is confirmed to still have unresolved conflicts
    /// (unmerged entries remain) — the merge is still pending, not stale.
    ConfirmedMergePending,
    /// A precondition (identity, topology, residue, configuration, working
    /// tree status, digest match, or author availability) could not be
    /// confirmed safe before any write — no Git write was attempted.
    PreWriteRejected,
    /// Exit 0 but the postcondition did not hold, or the write's outcome
    /// (timeout, spawn failure, or any other uncertainty) could not be
    /// confirmed either way.
    PostWriteUncertain,
}

/// Performs the single write this Unit supports:
/// `git ... -C <original-checkout> merge --continue`. Implementations must
/// re-verify every precondition [`crate::manual_merge_resolution::ManualMergeResolutionCandidatePort`]
/// checks immediately before spawning — never trust a caller's earlier
/// read alone.
pub trait MergeContinuePort {
    fn continue_merge(&mut self, request: &MergeContinueRequest) -> MergeContinueOutcome;
}
