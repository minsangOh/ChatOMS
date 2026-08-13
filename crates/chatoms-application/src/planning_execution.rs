//! Orchestrates a single Claude Planning attempt end to end: the
//! `WorktreeReady -> Planning` transition (delegating to
//! `TaskService::start_planning` for the consent-and-transition contract),
//! and — once a caller has actually run a `ClaudePlanningExecutor` —
//! recording its outcome (delegating to `TaskService::record_planning_result`).
//!
//! Split into two services, [`PlanningExecutionStarter`] and
//! [`PlanningExecutionRecorder`], because the transition is fast and
//! DB-transactional while the actual provider run can take a long time: a
//! caller (the Tauri command layer) runs `begin` synchronously and
//! `run_and_record` on a background thread, so a concurrent cancellation
//! request can still reach the in-flight run.

use std::path::Path;

use chatoms_domain::TaskId;
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    planning::{ClaudePlanningExecutor, PlanningExecutionBrief, PlanningExecutionStartOutcome},
    process::CancellationSignal,
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::{
        FoundationRepository, GitIsolationStatus, PlanningResultOutcome, TaskBriefRecord,
    },
};

use crate::{
    error::ApplicationError,
    tasks::{RecordPlanningResultRequest, StartPlanningRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "user";
const CONSENT_REASON: &str = "task.planning.consent";
const RESULT_REASON: &str = "task.planning.result";

pub struct BeginPlanningExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginPlanningExecutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything a caller needs to actually run and then record a Claude
/// Planning attempt, once [`PlanningExecutionStarter::begin`] has committed
/// the `WorktreeReady -> Planning` transition. `task.version` is the *new*
/// (post-transition) version, which the eventual `run_and_record` call must
/// pass back as its `expected_version`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningExecutionInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub brief: TaskBriefRecord,
}

pub struct PlanningExecutionStarter<'a, R, T, C> {
    repository: &'a mut R,
    time: &'a mut T,
    capability: &'a mut C,
}

impl<'a, R, T, C> PlanningExecutionStarter<'a, R, T, C>
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
    /// `WorktreeReady -> Planning` transition via `TaskService::start_planning`
    /// and fetches the worktree path and brief the eventual provider run
    /// needs. On an unsupported capability, wrong task state, a stale
    /// version, or a missing isolation record, nothing is written and the
    /// task's state is left exactly as it was: "no execution, state
    /// preserved".
    pub fn begin(
        &mut self,
        request: BeginPlanningExecutionRequest,
    ) -> Result<PlanningExecutionInputs, ApplicationError> {
        let capabilities = self
            .capability
            .provider_capabilities()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Err(category_error(FailureCategory::Unsupported));
        }

        let task = TaskService::new(self.repository, self.time).start_planning(
            StartPlanningRequest::new(
                request.task_id,
                request.expected_version,
                ACTOR_KIND.to_owned(),
                CONSENT_REASON.to_owned(),
            ),
        )?;

        match self.load_execution_inputs(request.task_id) {
            Ok((worktree_path, brief)) => Ok(PlanningExecutionInputs {
                task,
                worktree_path,
                brief,
            }),
            Err(error) => {
                // The transition above already committed, so — unlike every
                // rejection path before it — this is no longer a "nothing
                // written" failure. `TaskService::start_planning` itself
                // already required a `WorktreeReady`-status isolation record
                // (which always carries a `worktree_path`) and a task can
                // only ever reach `WorktreeReady` with a `TaskBrief` already
                // attached (Unit 4a-1), so reaching here means an upstream
                // invariant broke, not a normal rejection. Mirror
                // `TaskService::recover_after_planning_persistence_failure`'s
                // "never leave the task silently stuck" fallback rather than
                // returning an error while the task sits unreachable in
                // `Planning`.
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

pub struct PlanningExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> PlanningExecutionRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs `executor` for the Planning attempt already started by
    /// [`PlanningExecutionStarter::begin`], then records its outcome via
    /// `TaskService::record_planning_result` atomically with the resulting
    /// state transition. Never leaves the task stuck in `Planning`: a
    /// post-transition preflight rejection or a genuine executor failure
    /// both fall back to `RecoveryRequired`, the same fallback
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
        X: ClaudePlanningExecutor,
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
    /// contains a panic anywhere in that call (the executor, the adapter it
    /// wraps, or this method's own bookkeeping) so a caller running this on
    /// a detached background thread never has the panic propagate out and
    /// skip whatever cleanup runs after this call returns.
    ///
    /// On a caught panic, this attempts the exact same `RecoveryRequired`
    /// fallback [`Self::run_and_record`] already uses for a non-panicking
    /// failure — reusing the existing atomic transition-plus-history path,
    /// so `Planning -> RecoveryRequired` is recorded and the lease is kept,
    /// exactly as if the executor had returned an ordinary error instead of
    /// panicking. The panic's payload is never inspected, formatted, or
    /// forwarded anywhere — not into this method's `Result`, not to a log,
    /// not to the database — only the caught/not-caught distinction is
    /// used. If the fallback write itself fails, this returns that failure
    /// without retrying or treating it as success; the task may be left in
    /// `Planning`, exactly the case `TaskService::reconcile_startup_planning`
    /// exists to recover on the next app startup.
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
        X: ClaudePlanningExecutor,
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
