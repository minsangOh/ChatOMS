//! Port boundary for running a single Claude Planning attempt whose stdin
//! body is a Context Package v1 package instead of the fixed `format_stdin`
//! template [`crate::planning::ClaudePlanningExecutor`] sends. Kept as a
//! wholly separate trait — not a parameter added to
//! [`crate::planning::ClaudePlanningExecutor::start_planning`] — so an
//! application-layer caller can depend on "run Claude Planning against a
//! Context Package v1 body" without ever constructing, naming, or importing
//! an assembled-package type: the brief and cancellation signal here are
//! exactly [`crate::planning::PlanningExecutionBrief`]/
//! [`crate::process::CancellationSignal`], the same types the legacy port
//! already uses, and assembly happens entirely inside the implementation.
//! This preserves the dependency-inversion boundary application code already
//! respects (depend on ports, never on infrastructure's assembler or its
//! `AssembledContextPackage` type).

use std::path::Path;

use crate::error::PortFailure;
use crate::planning::{PlanningExecutionBrief, PlanningExecutionStartOutcome};
use crate::process::CancellationSignal;

/// Runs a single Claude Planning attempt against `worktree` (read-only)
/// whose stdin is assembled from `brief` as a Context Package v1 body by the
/// implementation itself. Reuses [`crate::planning::PlanningExecutionResult`]/
/// [`PlanningExecutionStartOutcome`] unchanged: the safe, Task-state-machine
/// -ready result vocabulary a completed Context Package v1 Planning attempt
/// produces is identical to a legacy one, so no new outcome/result type is
/// introduced. Implementations must re-verify provider capability
/// immediately before every spawn rather than trusting an earlier cached
/// result, matching [`crate::planning::ClaudePlanningExecutor`].
pub trait ContextPackagePlanningExecutor {
    fn start_planning(
        &mut self,
        worktree: &Path,
        brief: PlanningExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<PlanningExecutionStartOutcome, PortFailure>;
}
