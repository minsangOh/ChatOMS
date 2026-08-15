//! Port boundary for running a single Claude Review attempt whose stdin body
//! is a Context Package v1 package instead of the fixed `format_stdin`
//! template [`crate::review::ClaudeReviewExecutor`] sends. Kept as a wholly
//! separate trait — not a parameter added to
//! [`crate::review::ClaudeReviewExecutor::start_review`] — so an
//! application-layer caller can depend on "run Claude Review against a
//! Context Package v1 body" without ever constructing, naming, or importing
//! an assembled-package type: the brief and cancellation signal here are
//! exactly [`crate::review::ReviewExecutionBrief`]/
//! [`crate::process::CancellationSignal`], the same types the legacy port
//! already uses, and assembly happens entirely inside the implementation.
//! This preserves the dependency-inversion boundary application code already
//! respects (depend on ports, never on infrastructure's assembler or its
//! `AssembledContextPackage` type). Mirrors
//! [`crate::context_package_implementation::ContextPackageImplementationExecutor`]'s
//! shape exactly.

use std::path::Path;

use crate::error::PortFailure;
use crate::process::CancellationSignal;
use crate::review::{ReviewExecutionBrief, ReviewExecutionStartOutcome};

/// Runs a single Claude Review attempt against `worktree` (read-only) whose
/// stdin is assembled from `brief` as a Context Package v1 body by the
/// implementation itself. Reuses
/// [`crate::review::ReviewExecutionResult`]/[`ReviewExecutionStartOutcome`]
/// unchanged: the safe, Task-state-machine-ready result vocabulary a
/// completed Context Package v1 Review attempt produces is identical to a
/// legacy one, so no new outcome/result type is introduced.
/// `brief.diff_text` is the same ephemeral, already-bounded current worktree
/// diff the legacy port already carries — this trait introduces no new diff
/// source. Implementations must re-verify provider capability immediately
/// before every spawn rather than trusting an earlier cached result,
/// matching [`crate::review::ClaudeReviewExecutor`].
pub trait ContextPackageReviewExecutor {
    fn start_review(
        &mut self,
        worktree: &Path,
        brief: ReviewExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ReviewExecutionStartOutcome, PortFailure>;
}
