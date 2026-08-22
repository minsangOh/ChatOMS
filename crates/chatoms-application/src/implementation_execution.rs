//! Orchestrates a single Claude Implementation attempt end to end: the
//! `AwaitingDesignApproval -> Implementing` transition (delegating to
//! `TaskService::start_implementation` for the consent-and-transition
//! contract), and — once a caller has actually run a
//! `ClaudeImplementationExecutor` — recording its outcome (delegating to
//! `TaskService::record_implementation_result`).
//!
//! Split into two services, [`ImplementationExecutionStarter`] and
//! [`ImplementationExecutionRecorder`], mirroring
//! [`crate::planning_execution`]'s split for the same reason: the
//! transition is fast and DB-transactional while the actual provider run
//! can take a long time.
//!
//! Unlike [`crate::planning_execution::PlanningExecutionStarter::begin`],
//! this Starter validates that the required evidence (a `WorktreeReady`
//! isolation record and a `Completed` Claude Planning result carrying
//! non-empty plan text) exists *before* committing the
//! `AwaitingDesignApproval -> Implementing` transition, not after. A
//! write-capable Implementation run has no safe way to unwind partial
//! filesystem changes, so this Unit's approved contract requires that
//! missing evidence never advance the task's state or record a consent in
//! the first place — there is no post-transition `RecoveryRequired`
//! fallback here because nothing is committed until every precondition is
//! confirmed.

use std::path::Path;

use chatoms_domain::TaskId;
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    implementation::{
        ImplementationExecutionBrief, ImplementationExecutionStartOutcome,
        PolicyGatedClaudeImplementationExecutor,
    },
    process::CancellationSignal,
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::{
        FoundationRepository, GitIsolationStatus, ImplementationResultOutcome,
        PlanningResultOutcome, TaskBriefRecord,
    },
};

use crate::{
    error::ApplicationError,
    policy_engine::{PolicyPermit, require_provider_implementation_permit},
    tasks::{RecordImplementationResultRequest, StartImplementationRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "user";
const CONSENT_REASON: &str = "task.implementation.consent";
const RESULT_REASON: &str = "task.implementation.result";

pub struct BeginImplementationExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginImplementationExecutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything a caller needs to actually run and then record a Claude
/// Implementation attempt, once [`ImplementationExecutionStarter::begin`]
/// has committed the `AwaitingDesignApproval -> Implementing` transition.
/// `task.version` is the *new* (post-transition) version, which the
/// eventual `run_and_record` call must pass back as its `expected_version`.
pub struct ImplementationExecutionInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub brief: TaskBriefRecord,
    pub plan_text: String,
    pub policy_permit: PolicyPermit,
}

pub struct ImplementationExecutionStarter<'a, R, T, C, F> {
    repository: &'a mut R,
    time: &'a mut T,
    capability: &'a mut C,
    filesystem: &'a mut F,
}

impl<'a, R, T, C, F> ImplementationExecutionStarter<'a, R, T, C, F>
where
    R: FoundationRepository,
    T: TimeProvider,
    C: ProviderCapabilityPort,
    F: FilesystemIdentityPort,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        time: &'a mut T,
        capability: &'a mut C,
        filesystem: &'a mut F,
    ) -> Self {
        Self {
            repository,
            time,
            capability,
            filesystem,
        }
    }

    /// Loads and validates the evidence a write-capable run requires — a
    /// `WorktreeReady` isolation
    /// record, a `Completed` Claude Planning result with non-empty plan
    /// text, and the `TaskBrief` — and only once every one of those is
    /// confirmed present, evaluates the Provider Implementation policy, and
    /// fresh-checks Claude capability before it commits the
    /// `AwaitingDesignApproval -> Implementing` transition via
    /// `TaskService::start_implementation`. On an unsupported capability,
    /// missing/invalid evidence, wrong task state, or a stale version,
    /// nothing is written and the task's state is left exactly as it was:
    /// "no execution, no consent, state preserved". Policy rejection occurs
    /// before the capability probe can start any provider process.
    pub fn begin(
        &mut self,
        request: BeginImplementationExecutionRequest,
    ) -> Result<ImplementationExecutionInputs, ApplicationError> {
        let (worktree_path, brief, plan_text) = self.load_execution_evidence(request.task_id)?;
        let policy_permit = require_provider_implementation_permit(
            self.repository,
            self.filesystem,
            request.task_id,
            request.expected_version,
        )?;

        let capabilities = self
            .capability
            .provider_capabilities()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Err(category_error(FailureCategory::Unsupported));
        }

        let task = TaskService::new(self.repository, self.time).start_implementation(
            StartImplementationRequest::new(
                request.task_id,
                request.expected_version,
                ACTOR_KIND.to_owned(),
                CONSENT_REASON.to_owned(),
            ),
        )?;

        Ok(ImplementationExecutionInputs {
            task,
            worktree_path,
            brief,
            plan_text,
            policy_permit,
        })
    }

    /// Loads and validates the read-only evidence a write-capable run
    /// requires, without writing anything. Returns a typed
    /// `ApplicationError` (never starting a subprocess or touching task
    /// state) when the isolation record is missing/not `WorktreeReady`,
    /// the stored Claude Planning result is missing, did not `Complete`, or
    /// carries no plan text, or the `TaskBrief` is missing.
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

pub struct ImplementationExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> ImplementationExecutionRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs `executor` for the Implementation attempt already started by
    /// [`ImplementationExecutionStarter::begin`], then records its outcome
    /// via `TaskService::record_implementation_result` atomically with the
    /// resulting state transition. Never leaves the task stuck in
    /// `Implementing`: a post-transition preflight rejection or a genuine
    /// executor failure both fall back to `RecoveryRequired`, the same
    /// fallback `TaskService` already uses when its own atomic result write
    /// fails.
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
        X: PolicyGatedClaudeImplementationExecutor,
    {
        let outcome = executor.start_implementation(
            task_id,
            expected_version,
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
    /// the adapter it wraps, or this method's own bookkeeping) so a caller
    /// running this on a detached background thread never has the panic
    /// propagate out and skip whatever cleanup runs after this call
    /// returns.
    ///
    /// On a caught panic, this attempts the exact same `RecoveryRequired`
    /// fallback [`Self::run_and_record`] already uses for a non-panicking
    /// failure — reusing the existing atomic transition-plus-history path,
    /// so `Implementing -> RecoveryRequired` is recorded and the lease is
    /// kept, exactly as if the executor had returned an ordinary error
    /// instead of panicking. The panic's payload is never inspected,
    /// formatted, or forwarded anywhere — not into this method's `Result`,
    /// not to a log, not to the database — only the caught/not-caught
    /// distinction is used. If the fallback write itself fails, this
    /// returns that failure without retrying or treating it as success;
    /// the task may be left in `Implementing`.
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
        X: PolicyGatedClaudeImplementationExecutor,
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
