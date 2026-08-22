//! Orchestrates a single Context Package v1 Claude Planning attempt end to
//! end, mirroring `crate::planning_execution` exactly in shape but for the
//! Context Package v1 activation path — that module is not modified or
//! extended by this one.
//!
//! The `WorktreeReady -> Planning` transition delegates to
//! `TaskService::start_context_package_planning`, which (unlike
//! `TaskService::start_planning`) never creates a new provider-transmission
//! consent: it requires an already-prepared exact `(task_id, Claude,
//! Planning, expected_version, ContextPackageV1)` consent and its FK-bound
//! manifest (see `TaskService::prepare_planning_context_package`) and fails
//! closed, leaving the task `WorktreeReady`, if either is missing. Recording
//! the eventual outcome reuses `TaskService::record_planning_result`
//! completely unmodified — a Context Package v1 Planning attempt's safe,
//! reduced result is indistinguishable in shape from a legacy one.
//!
//! Split into two services, [`ContextPackagePlanningExecutionStarter`] and
//! [`ContextPackagePlanningExecutionRecorder`], for the same reason
//! `planning_execution.rs` is: the transition is fast and DB-transactional
//! while the actual provider run can take a long time, and both run
//! against the same shared `PlanningRunRegistry` cancellation handle a
//! caller (the Tauri command layer) registers between the two calls.

use std::path::Path;

use chatoms_domain::TaskId;
use chatoms_ports::{
    TimeProvider,
    context_package_planning::ContextPackagePlanningExecutor,
    error::FailureCategory,
    planning::{PlanningExecutionBrief, PlanningExecutionStartOutcome},
    process::CancellationSignal,
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::{
        FoundationRepository, GitIsolationStatus, PlanningResultOutcome, TaskBriefRecord,
    },
};

use crate::{
    error::ApplicationError,
    tasks::{
        RecordPlanningResultRequest, StartContextPackagePlanningRequest, TaskService, TaskView,
    },
};

const ACTOR_KIND: &str = "user";
const TRANSITION_REASON: &str = "task.planning.context_package.transition";
const RESULT_REASON: &str = "task.planning.result";

pub struct BeginContextPackagePlanningExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginContextPackagePlanningExecutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything a caller needs to actually run and then record a Context
/// Package v1 Claude Planning attempt, once
/// [`ContextPackagePlanningExecutionStarter::begin`] has committed the
/// `WorktreeReady -> Planning` transition. `task.version` is the *new*
/// (post-transition) version, which the eventual `run_and_record` call must
/// pass back as its `expected_version` — identical shape to
/// `crate::planning_execution::PlanningExecutionInputs`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPackagePlanningExecutionInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub brief: TaskBriefRecord,
}

pub struct ContextPackagePlanningExecutionStarter<'a, R, T, C> {
    repository: &'a mut R,
    time: &'a mut T,
    capability: &'a mut C,
}

impl<'a, R, T, C> ContextPackagePlanningExecutionStarter<'a, R, T, C>
where
    R: FoundationRepository,
    T: TimeProvider,
    C: ProviderCapabilityPort,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T, capability: &'a mut C) -> Self {
        Self {
            repository,
            time,
            capability,
        }
    }

    /// Fresh-checks Claude capability, then — only if supported — commits the
    /// `WorktreeReady -> Planning` transition via
    /// `TaskService::start_context_package_planning` (which itself verifies
    /// the Context Package v1 consent/manifest pair is already prepared) and
    /// fetches the worktree path and brief the eventual provider run needs.
    /// On an unsupported capability, wrong task state, a stale version, a
    /// missing isolation record, or a missing/partial Context Package v1
    /// preparation, nothing is written and the task's state is left exactly
    /// as it was: "no execution, state preserved".
    pub fn begin(
        &mut self,
        request: BeginContextPackagePlanningExecutionRequest,
    ) -> Result<ContextPackagePlanningExecutionInputs, ApplicationError> {
        let capabilities = self
            .capability
            .provider_capabilities()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Err(category_error(FailureCategory::Unsupported));
        }

        let task = TaskService::new(self.repository, self.time).start_context_package_planning(
            StartContextPackagePlanningRequest::new(
                request.task_id,
                request.expected_version,
                ACTOR_KIND.to_owned(),
                TRANSITION_REASON.to_owned(),
            ),
        )?;

        match self.load_execution_inputs(request.task_id) {
            Ok((worktree_path, brief)) => Ok(ContextPackagePlanningExecutionInputs {
                task,
                worktree_path,
                brief,
            }),
            Err(error) => {
                // The transition above already committed, so this is no
                // longer a "nothing written" failure — mirrors
                // `PlanningExecutionStarter::begin`'s identical fallback for
                // the identical reason: `start_context_package_planning`
                // already required a `WorktreeReady`-status isolation
                // record (which always carries a `worktree_path`) and a
                // `TaskBrief` (Unit 4a-1's invariant), so reaching here means
                // an upstream invariant broke, not a normal rejection.
                self.fall_back_to_recovery_required(request.task_id, task.version);
                Err(error)
            }
        }
    }

    fn load_execution_inputs(
        &mut self,
        task_id: TaskId,
    ) -> Result<(String, TaskBriefRecord), ApplicationError> {
        let isolation = self
            .repository
            .get_task_isolation(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        let worktree_path = isolation
            .worktree_path
            .filter(|_| isolation.status == GitIsolationStatus::WorktreeReady)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let brief = self
            .repository
            .get_task_brief(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        Ok((worktree_path, brief))
    }

    fn fall_back_to_recovery_required(&mut self, task_id: TaskId, expected_version: u64) {
        let Ok(started_at_ms) = self.time.now_ms() else {
            return;
        };
        let _ = TaskService::new(self.repository, self.time).record_planning_result(
            RecordPlanningResultRequest::new(
                task_id,
                expected_version,
                PlanningResultOutcome::RecoveryRequired,
                None,
                None,
                None,
                started_at_ms,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ),
        );
    }
}

pub struct ContextPackagePlanningExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> ContextPackagePlanningExecutionRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs `executor` for the Context Package v1 Planning attempt already
    /// started by [`ContextPackagePlanningExecutionStarter::begin`], then
    /// records its outcome via the exact same, unmodified
    /// `TaskService::record_planning_result` the legacy path uses,
    /// atomically with the resulting state transition. Never leaves the
    /// task stuck in `Planning`: an assembly rejection folded by the
    /// executor into `PreflightRejected` (see
    /// `chatoms_infrastructure::claude_planning`'s
    /// `ContextPackagePlanningExecutor` impl) and a genuine executor
    /// failure both fall back to `RecoveryRequired`, the same fallback
    /// `TaskService` already uses when its own atomic result write fails.
    #[allow(clippy::too_many_arguments)]
    pub fn run_and_record<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        brief: &TaskBriefRecord,
        started_at_ms: i64,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ContextPackagePlanningExecutor,
    {
        let outcome = executor.start_planning(
            Path::new(worktree_path),
            PlanningExecutionBrief {
                requirements: &brief.requirements,
                completion_criteria: &brief.completion_criteria,
                prohibited_scope: &brief.prohibited_scope,
            },
            cancellation,
        );

        let (result_outcome, exit_code, turn_count, plan_text) = match outcome {
            Ok(PlanningExecutionStartOutcome::Completed(result)) => (
                result.outcome,
                result.exit_code,
                result.turn_count,
                result.plan_text,
            ),
            Ok(PlanningExecutionStartOutcome::PreflightRejected) | Err(_) => {
                (PlanningResultOutcome::RecoveryRequired, None, None, None)
            }
        };

        TaskService::new(self.repository, self.time).record_planning_result(
            RecordPlanningResultRequest::new(
                task_id,
                expected_version,
                result_outcome,
                exit_code,
                turn_count,
                plan_text,
                started_at_ms,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ),
        )
    }

    /// Runs `executor` the same as [`Self::run_and_record`], but additionally
    /// contains a panic anywhere in that call (the executor, the assembler
    /// or adapter it wraps, or this method's own bookkeeping) so a caller
    /// running this on a detached background thread never has the panic
    /// propagate out and skip whatever cleanup runs after this call
    /// returns. Mirrors
    /// `crate::planning_execution::PlanningExecutionRecorder::run_and_record_with_panic_containment`
    /// exactly: on a caught panic, this attempts the same `RecoveryRequired`
    /// fallback [`Self::run_and_record`] already uses for a non-panicking
    /// failure, never inspects/forwards the panic payload, and if the
    /// fallback write itself fails, returns that failure without retrying
    /// or treating it as success.
    #[allow(clippy::too_many_arguments)]
    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        brief: &TaskBriefRecord,
        started_at_ms: i64,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ContextPackagePlanningExecutor,
    {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_and_record(
                task_id,
                expected_version,
                worktree_path,
                brief,
                started_at_ms,
                executor,
                cancellation,
            )
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => TaskService::new(self.repository, self.time).record_planning_result(
                RecordPlanningResultRequest::new(
                    task_id,
                    expected_version,
                    PlanningResultOutcome::RecoveryRequired,
                    None,
                    None,
                    None,
                    started_at_ms,
                    ACTOR_KIND.to_owned(),
                    RESULT_REASON.to_owned(),
                ),
            ),
        }
    }
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
