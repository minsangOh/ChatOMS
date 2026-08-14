//! Port boundary for running a single Claude Planning attempt. Kept
//! separate from [`crate::provider::ProviderCapabilityPort`] (capability
//! reporting only) so an application-layer orchestrator can depend on "run
//! Claude Planning" without depending on the infrastructure crate's process
//! and redaction plumbing that actually implements it.

use std::path::Path;

use crate::error::PortFailure;
use crate::process::CancellationSignal;
use crate::repository::PlanningResultOutcome;

/// The three fixed `TaskBrief` fields a Claude Planning attempt is run
/// against, borrowed rather than owned so callers do not need to clone
/// brief text just to start a run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanningExecutionBrief<'a> {
    pub requirements: &'a str,
    pub completion_criteria: &'a str,
    pub prohibited_scope: &'a str,
}

/// A completed Claude Planning attempt reduced to the safe,
/// Task-state-machine-ready vocabulary already used by
/// [`crate::repository::TaskPlanningResultRecord`]. `plan_text` is `Some`
/// only when `outcome` is `Completed`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningExecutionResult {
    pub outcome: PlanningResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub plan_text: Option<String>,
}

/// Result of attempting to start a Claude Planning invocation.
/// `PreflightRejected` means a fresh trust/compatibility/login/preflight
/// gate failed immediately before spawn, so no subprocess was started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanningExecutionStartOutcome {
    Completed(PlanningExecutionResult),
    PreflightRejected,
}

/// Runs a single Claude Planning attempt against `worktree` (read-only) using
/// `brief`, cooperatively cancellable via `cancellation`. Implementations
/// re-verify provider capability immediately before every spawn rather than
/// trusting an earlier cached result.
pub trait ClaudePlanningExecutor {
    fn start_planning(
        &mut self,
        worktree: &Path,
        brief: PlanningExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<PlanningExecutionStartOutcome, PortFailure>;
}
