//! Port boundary for running a single Claude Implementation attempt. Kept
//! separate from [`crate::provider::ProviderCapabilityPort`] (capability
//! reporting only) so an application-layer orchestrator can depend on "run
//! Claude Implementation" without depending on the infrastructure crate's
//! process and redaction plumbing that actually implements it. Mirrors
//! [`crate::planning::ClaudePlanningExecutor`]'s shape.

use std::path::Path;

use crate::error::PortFailure;
use crate::process::CancellationSignal;
use crate::repository::ImplementationResultOutcome;

/// The three fixed `TaskBrief` fields plus the previously stored Claude
/// Planning result text a Claude Implementation attempt is run against,
/// borrowed rather than owned so callers do not need to clone this text
/// just to start a run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImplementationExecutionBrief<'a> {
    pub requirements: &'a str,
    pub completion_criteria: &'a str,
    pub prohibited_scope: &'a str,
    pub plan_text: &'a str,
}

/// A completed Claude Implementation attempt reduced to the safe,
/// Task-state-machine-ready vocabulary already used by
/// [`crate::repository::TaskImplementationResultRecord`]. Unlike
/// [`crate::planning::PlanningExecutionResult`], this carries no content
/// field: [`crate::repository::TaskImplementationResultRecord`] never
/// stores one, so nothing at or below this port boundary needs to either.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImplementationExecutionResult {
    pub outcome: ImplementationResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
}

/// Result of attempting to start a Claude Implementation invocation.
/// `PreflightRejected` means a fresh trust/compatibility/login/preflight
/// gate failed, or the composed stdin payload was rejected as oversized,
/// immediately before spawn — either way no subprocess was started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImplementationExecutionStartOutcome {
    Completed(ImplementationExecutionResult),
    PreflightRejected,
}

/// Runs a single Claude Implementation attempt against `worktree`
/// (read+write) using `brief`, cooperatively cancellable via
/// `cancellation`. Implementations re-verify provider capability
/// immediately before every spawn rather than trusting an earlier cached
/// result.
pub trait ClaudeImplementationExecutor {
    fn start_implementation(
        &mut self,
        worktree: &Path,
        brief: ImplementationExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ImplementationExecutionStartOutcome, PortFailure>;
}
