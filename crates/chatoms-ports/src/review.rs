//! Port boundary for running a single Claude Review attempt. Kept separate
//! from [`crate::provider::ProviderCapabilityPort`] (capability reporting
//! only) so an application-layer orchestrator can depend on "run Claude
//! Review" without depending on the infrastructure crate's process and
//! redaction plumbing that actually implements it. Mirrors
//! [`crate::planning::ClaudePlanningExecutor`]'s shape (Review, like
//! Planning, is read-only and its
//! [`crate::repository::TaskReviewResultRecord`] stores a content field),
//! not [`crate::implementation::ClaudeImplementationExecutor`]'s.

use std::path::Path;

use crate::error::PortFailure;
use crate::process::CancellationSignal;
use crate::repository::ReviewResultOutcome;

/// The three fixed `TaskBrief` fields plus the ephemeral, already-bounded
/// current worktree diff (from
/// [`crate::diff::WorktreeDiffPort::current_diff`]) a Claude Review attempt
/// is run against, borrowed rather than owned so callers do not need to
/// clone this text just to start a run. Deliberately excludes the stored
/// Claude Planning result text — this Unit's first Review adapter input
/// does not carry it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReviewExecutionBrief<'a> {
    pub requirements: &'a str,
    pub completion_criteria: &'a str,
    pub prohibited_scope: &'a str,
    pub diff_text: &'a str,
}

/// A completed Claude Review attempt reduced to the safe,
/// Task-state-machine-ready vocabulary already used by
/// [`crate::repository::TaskReviewResultRecord`]. `review_text` is masked
/// and size-bounded by the caller before this record is built (see
/// `chatoms_infrastructure::redaction::SecretRedactor`) and is `Some` only
/// when `outcome` is `Completed`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewExecutionResult {
    pub outcome: ReviewResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub review_text: Option<String>,
}

/// Result of attempting to start a Claude Review invocation.
/// `PreflightRejected` means a fresh trust/compatibility/login/preflight
/// gate failed, or the composed stdin payload was rejected as oversized,
/// immediately before spawn — either way no subprocess was started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewExecutionStartOutcome {
    Completed(ReviewExecutionResult),
    PreflightRejected,
}

/// Runs a single Claude Review attempt against `worktree` (read-only) using
/// `brief`, cooperatively cancellable via `cancellation`. Implementations
/// re-verify provider capability immediately before every spawn rather than
/// trusting an earlier cached result.
pub trait ClaudeReviewExecutor {
    fn start_review(
        &mut self,
        worktree: &Path,
        brief: ReviewExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ReviewExecutionStartOutcome, PortFailure>;
}
