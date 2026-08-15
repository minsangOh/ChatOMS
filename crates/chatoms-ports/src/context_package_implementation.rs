//! Port boundary for running a single Claude Implementation attempt whose
//! stdin body is a Context Package v1 package instead of the fixed
//! `format_stdin` template [`crate::implementation::ClaudeImplementationExecutor`]
//! sends. Kept as a wholly separate trait — not a parameter added to
//! [`crate::implementation::ClaudeImplementationExecutor::start_implementation`]
//! — so an application-layer caller can depend on "run Claude Implementation
//! against a Context Package v1 body" without ever constructing, naming, or
//! importing an assembled-package type: the brief and cancellation signal
//! here are exactly [`crate::implementation::ImplementationExecutionBrief`]/
//! [`crate::process::CancellationSignal`], the same types the legacy port
//! already uses, and assembly happens entirely inside the implementation.
//! This preserves the dependency-inversion boundary application code already
//! respects (depend on ports, never on infrastructure's assembler or its
//! `AssembledContextPackage` type). Mirrors
//! [`crate::context_package_planning::ContextPackagePlanningExecutor`]'s
//! shape exactly.

use std::path::Path;

use crate::error::PortFailure;
use crate::implementation::{ImplementationExecutionBrief, ImplementationExecutionStartOutcome};
use crate::process::CancellationSignal;

/// Runs a single Claude Implementation attempt against `worktree`
/// (read+write) whose stdin is assembled from `brief` as a Context Package
/// v1 body by the implementation itself. Reuses
/// [`crate::implementation::ImplementationExecutionResult`]/
/// [`ImplementationExecutionStartOutcome`] unchanged: the safe,
/// Task-state-machine-ready result vocabulary a completed Context Package v1
/// Implementation attempt produces is identical to a legacy one, so no new
/// outcome/result type is introduced. Implementations must re-verify
/// provider capability immediately before every spawn rather than trusting
/// an earlier cached result, matching
/// [`crate::implementation::ClaudeImplementationExecutor`].
pub trait ContextPackageImplementationExecutor {
    fn start_implementation(
        &mut self,
        worktree: &Path,
        brief: ImplementationExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ImplementationExecutionStartOutcome, PortFailure>;
}
