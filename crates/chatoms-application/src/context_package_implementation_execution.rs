//! Orchestrates a single Context Package v1 Claude Implementation attempt
//! end to end, mirroring `crate::implementation_execution`'s ordering — all
//! required evidence is loaded and validated *before* any state is
//! committed, not `crate::context_package_planning_execution`'s "commit
//! first, fall back after" shape — but delegating the transition itself to
//! this Unit's own `TaskService::start_context_package_implementation`.
//! Neither `crate::implementation_execution` nor
//! `crate::context_package_planning_execution` is modified by this module.
//!
//! `TaskService::start_context_package_implementation` already re-verifies,
//! read-only and inside its own atomic transaction boundary, every
//! precondition this Starter loads first (task state/version, isolation, the
//! stored Claude Planning result, the `TaskBrief`, and the exact `(task_id,
//! Claude, Implementation, expected_version, ContextPackageV1)`
//! consent/manifest pair) — so if this Starter's own read succeeds but that
//! call still fails (a concurrent state change, or the context package
//! simply not yet prepared), nothing has been committed and the task is
//! left exactly `AwaitingDesignApproval`, matching
//! `ImplementationExecutionStarter::begin`'s "no execution, no consent,
//! state preserved" contract for the identical write-capable reason: a
//! partial commit before every precondition is confirmed has no safe way to
//! unwind.
//!
//! Split into two services, [`ContextPackageImplementationExecutionStarter`]
//! and [`ContextPackageImplementationExecutionRecorder`], for the same
//! reason `implementation_execution.rs` is: the transition is fast and
//! DB-transactional while the actual provider run can take a long time, and
//! both run against the same shared `ImplementationRunRegistry`
//! cancellation handle a caller (the Tauri command layer) registers between
//! the two calls.

use std::path::Path;

use chatoms_domain::TaskId;
use chatoms_ports::{
    TimeProvider,
    context_package_implementation::ContextPackageImplementationExecutor,
    error::FailureCategory,
    implementation::{ImplementationExecutionBrief, ImplementationExecutionStartOutcome},
    process::CancellationSignal,
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::{
        FoundationRepository, GitIsolationStatus, ImplementationResultOutcome,
        PlanningResultOutcome, TaskBriefRecord,
    },
};

use crate::{
    error::ApplicationError,
    tasks::{
        RecordImplementationResultRequest, StartContextPackageImplementationRequest, TaskService,
        TaskView,
    },
};

const ACTOR_KIND: &str = "user";
const TRANSITION_REASON: &str = "task.implementation.context_package.transition";
const RESULT_REASON: &str = "task.implementation.result";

pub struct BeginContextPackageImplementationExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginContextPackageImplementationExecutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything a caller needs to actually run and then record a Context
/// Package v1 Claude Implementation attempt, once
/// [`ContextPackageImplementationExecutionStarter::begin`] has committed the
/// `AwaitingDesignApproval -> Implementing` transition. `task.version` is
/// the *new* (post-transition) version, which the eventual `run_and_record`
/// call must pass back as its `expected_version` — identical shape to
/// `crate::implementation_execution::ImplementationExecutionInputs`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPackageImplementationExecutionInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub brief: TaskBriefRecord,
    pub plan_text: String,
}

pub struct ContextPackageImplementationExecutionStarter<'a, R, T, C> {
    repository: &'a mut R,
    time: &'a mut T,
    capability: &'a mut C,
}

impl<'a, R, T, C> ContextPackageImplementationExecutionStarter<'a, R, T, C>
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

    /// Fresh-checks Claude capability, then loads and validates the
    /// evidence a write-capable run requires — a `WorktreeReady` isolation
    /// record, a `Completed` Claude Planning result with non-empty plan
    /// text, and the `TaskBrief` — exactly like
    /// `ImplementationExecutionStarter::begin`, and only once every one of
    /// those is confirmed present does it commit the
    /// `AwaitingDesignApproval -> Implementing` transition via
    /// `TaskService::start_context_package_implementation` (which
    /// additionally requires, inside its own atomic transaction, the exact
    /// Context Package v1 consent/manifest pair). On an unsupported
    /// capability, missing/invalid evidence, wrong task state, a stale
    /// version, or a missing/partial Context Package v1 preparation,
    /// nothing is written and the task's state is left exactly as it was:
    /// "no execution, no consent, state preserved".
    pub fn begin(
        &mut self,
        request: BeginContextPackageImplementationExecutionRequest,
    ) -> Result<ContextPackageImplementationExecutionInputs, ApplicationError> {
        let capabilities = self
            .capability
            .provider_capabilities()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Err(category_error(FailureCategory::Unsupported));
        }

        let (worktree_path, brief, plan_text) = self.load_execution_evidence(request.task_id)?;

        let task = TaskService::new(self.repository, self.time)
            .start_context_package_implementation(StartContextPackageImplementationRequest::new(
                request.task_id,
                request.expected_version,
                ACTOR_KIND.to_owned(),
                TRANSITION_REASON.to_owned(),
            ))?;

        Ok(ContextPackageImplementationExecutionInputs {
            task,
            worktree_path,
            brief,
            plan_text,
        })
    }

    /// Loads and validates the read-only evidence a write-capable run
    /// requires, without writing anything. Returns a typed
    /// `ApplicationError` (never starting a subprocess or touching task
    /// state) when the isolation record is missing/not `WorktreeReady`,
    /// the stored Claude Planning result is missing, did not `Complete`, or
    /// carries no plan text, or the `TaskBrief` is missing. Deliberately
    /// duplicates the shape of
    /// `crate::implementation_execution::ImplementationExecutionStarter`'s
    /// private `load_execution_evidence` rather than sharing it (that
    /// method is private to `crate::implementation_execution`, which this
    /// Unit does not modify).
    fn load_execution_evidence(
        &mut self,
        task_id: TaskId,
    ) -> Result<(String, TaskBriefRecord, String), ApplicationError> {
        let isolation = self
            .repository
            .get_task_isolation(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        let worktree_path = isolation
            .worktree_path
            .filter(|_| isolation.status == GitIsolationStatus::WorktreeReady)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;

        let planning_result = self
            .repository
            .get_task_planning_result(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if planning_result.outcome != PlanningResultOutcome::Completed {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let plan_text = planning_result
            .plan_text
            .filter(|text| !text.is_empty())
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;

        let brief = self
            .repository
            .get_task_brief(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;

        Ok((worktree_path, brief, plan_text))
    }
}

pub struct ContextPackageImplementationExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> ContextPackageImplementationExecutionRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs `executor` for the Context Package v1 Implementation attempt
    /// already started by
    /// [`ContextPackageImplementationExecutionStarter::begin`], then
    /// records its outcome via the exact same, unmodified
    /// `TaskService::record_implementation_result` the legacy path uses,
    /// atomically with the resulting state transition. Never leaves the
    /// task stuck in `Implementing`: an assembly rejection folded by the
    /// executor into `PreflightRejected` (see
    /// `chatoms_infrastructure::claude_implementation`'s
    /// `ContextPackageImplementationExecutor` impl) and a genuine executor
    /// failure both fall back to `RecoveryRequired`, the same fallback
    /// `TaskService` already uses when its own atomic result write fails.
    #[allow(clippy::too_many_arguments)]
    pub fn run_and_record<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        brief: &TaskBriefRecord,
        plan_text: &str,
        started_at_ms: i64,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ContextPackageImplementationExecutor,
    {
        let outcome = executor.start_implementation(
            Path::new(worktree_path),
            ImplementationExecutionBrief {
                requirements: &brief.requirements,
                completion_criteria: &brief.completion_criteria,
                prohibited_scope: &brief.prohibited_scope,
                plan_text,
            },
            cancellation,
        );

        let (result_outcome, exit_code, turn_count) = match outcome {
            Ok(ImplementationExecutionStartOutcome::Completed(result)) => {
                (result.outcome, result.exit_code, result.turn_count)
            }
            Ok(ImplementationExecutionStartOutcome::PreflightRejected) | Err(_) => {
                (ImplementationResultOutcome::RecoveryRequired, None, None)
            }
        };

        TaskService::new(self.repository, self.time).record_implementation_result(
            RecordImplementationResultRequest::new(
                task_id,
                expected_version,
                result_outcome,
                exit_code,
                turn_count,
                started_at_ms,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ),
        )
    }

    /// Runs `executor` the same as [`Self::run_and_record`], but
    /// additionally contains a panic anywhere in that call (the executor,
    /// the assembler or adapter it wraps, or this method's own bookkeeping)
    /// so a caller running this on a detached background thread never has
    /// the panic propagate out and skip whatever cleanup runs after this
    /// call returns. Mirrors
    /// `crate::implementation_execution::ImplementationExecutionRecorder::run_and_record_with_panic_containment`
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
        plan_text: &str,
        started_at_ms: i64,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ContextPackageImplementationExecutor,
    {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_and_record(
                task_id,
                expected_version,
                worktree_path,
                brief,
                plan_text,
                started_at_ms,
                executor,
                cancellation,
            )
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => TaskService::new(self.repository, self.time).record_implementation_result(
                RecordImplementationResultRequest::new(
                    task_id,
                    expected_version,
                    ImplementationResultOutcome::RecoveryRequired,
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
