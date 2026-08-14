//! Orchestrates one Testing batch attempt end to end: running every
//! Cargo-only validation command already approved for the task's current
//! version (see `crate::validation_commands` for the separate read-only
//! discovery/approval flow this depends on), in the fixed `Format -> Lint ->
//! Typecheck -> Test -> Build` order, and recording each attempt's
//! already-safe result — never raw stdout/stderr — atomically with the
//! resulting `Testing -> Reviewing/Paused/RecoveryRequired` transition.
//!
//! Split into two services, [`TestingBatchStarter`] and
//! [`TestingBatchRecorder`], mirroring `crate::planning_execution`'s and
//! `crate::implementation_execution`'s split. Unlike those, Testing needs no
//! consent or state transition to "start" (the task is already `Testing`),
//! but [`TestingBatchStarter::begin`] still performs the same
//! read-only-precondition-then-nothing-written-on-failure validation those
//! Starters do: task state/version, a `WorktreeReady` isolation record, and
//! at least one approved validation command for the current version. An
//! empty approval set is *not* silently treated as "batch complete" (that
//! would report success without validating anything) and does not force any
//! state transition — it is a typed, state-preserving error so the same
//! `Testing` task can be retried once at least one command is approved
//! (e.g. once a future approval-UI Unit lets the user approve one).
//!
//! Only intermediate `Success` results (every approved command before the
//! last one) are appended without a state change, via the existing
//! `FoundationRepository::append_validation_command_result`. The batch's
//! final result — the last approved command succeeding, or the *first*
//! non-success/cancelled outcome at any position — is appended atomically
//! with the state transition via `FoundationRepository::
//! finalize_validation_command_batch` (through `TaskService::
//! finalize_validation_command_batch`), mirroring
//! `record_implementation_result`'s "result row + transition, one
//! transaction" contract. Once a command ends the batch, no later approved
//! command runs.
//!
//! An identity/argv binding rejection from the executor (a stored approval
//! no longer matching live reality) is treated the same as a missing
//! approval: no subprocess is spawned, no result row is appended, and the
//! task's state is left exactly as it was — a typed error the caller can
//! retry once the approval is fixed, not a `RecoveryRequired` fallback.

use std::path::Path;

use chatoms_domain::{TaskId, TaskState, ValidationCommandKind};
use chatoms_ports::{
    TimeProvider,
    error::{FailureCategory, PortFailure},
    process::CancellationSignal,
    repository::{
        FoundationRepository, GitIsolationStatus, ValidationCommandApprovalRecord,
        ValidationCommandResultAttempt, ValidationCommandResultOutcome,
    },
    validation_execution::{
        ValidationCommandExecutor, ValidationExecutionOutcome, ValidationExecutionStartOutcome,
    },
};

use crate::{
    error::ApplicationError,
    tasks::{FinalizeValidationCommandBatchRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "application";
const RESULT_REASON: &str = "task.testing.validation-result";

pub struct BeginTestingBatchRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginTestingBatchRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything [`TestingBatchRecorder::run_and_record`] needs: the task
/// (unchanged — Testing needs no "start" transition), its worktree path,
/// and every approved validation command for the current version, already
/// ordered `Format -> Lint -> Typecheck -> Test -> Build`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestingBatchInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub approvals: Vec<ValidationCommandApprovalRecord>,
}

pub struct TestingBatchStarter<'a, R> {
    repository: &'a mut R,
}

impl<'a, R> TestingBatchStarter<'a, R>
where
    R: FoundationRepository,
{
    #[must_use]
    pub const fn new(repository: &'a mut R) -> Self {
        Self { repository }
    }

    /// Read-only: verifies the task is `Testing` at `expected_version`,
    /// resolves its `WorktreeReady` worktree path, and loads every approved
    /// validation command for that exact version — ordered by the fixed
    /// `ValidationCommandKind::ALL` sequence, never by insertion or storage
    /// order. Never writes anything. A wrong task state, a stale version, a
    /// missing/not-ready isolation record, or an empty approval set are all
    /// typed errors that leave the task exactly as it was.
    pub fn begin(
        &mut self,
        request: BeginTestingBatchRequest,
    ) -> Result<TestingBatchInputs, ApplicationError> {
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::Testing {
            return Err(category_error(FailureCategory::InvalidState));
        }

        let isolation = self
            .repository
            .get_task_isolation(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        let worktree_path = isolation
            .worktree_path
            .filter(|_| isolation.status == GitIsolationStatus::WorktreeReady)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;

        let mut approvals = self
            .repository
            .list_validation_command_approvals(request.task_id, request.expected_version)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if approvals.is_empty() {
            return Err(category_error(FailureCategory::NotFound));
        }
        approvals.sort_by_key(|approval| {
            ValidationCommandKind::ALL
                .iter()
                .position(|kind| *kind == approval.kind)
                .unwrap_or(usize::MAX)
        });

        Ok(TestingBatchInputs {
            task: TaskView::from(&task),
            worktree_path,
            approvals,
        })
    }
}

/// One command attempt's raw disposition, before it has been reduced to a
/// safe [`ValidationCommandResultOutcome`]. Kept as its own type so
/// [`TestingBatchRecorder::run_and_record`] and
/// [`TestingBatchRecorder::run_and_record_with_panic_containment`] can share
/// the exact same classification logic despite one of them wrapping the
/// executor call in `catch_unwind` and the other not.
enum CommandAttemptOutcome {
    Executed(Result<ValidationExecutionStartOutcome, PortFailure>),
    /// The executor call panicked; the payload is deliberately not carried
    /// here — only the fact that it panicked.
    Panicked,
}

pub struct TestingBatchRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> TestingBatchRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs every approval in `approvals`, in order, through `executor`.
    /// See the module docs for the exact append-vs-finalize split. Does
    /// *not* contain a panic in the executor call — see
    /// [`Self::run_and_record_with_panic_containment`] for that.
    pub fn run_and_record<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        approvals: &[ValidationCommandApprovalRecord],
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ValidationCommandExecutor,
    {
        if approvals.is_empty() {
            return Err(category_error(FailureCategory::NotFound));
        }
        let last_index = approvals.len() - 1;
        for (index, approval) in approvals.iter().enumerate() {
            let started_at_ms = self.now_ms()?;
            let outcome =
                executor.start_validation_command(Path::new(worktree_path), approval, cancellation);
            if let Some(view) = self.process_attempt(
                task_id,
                expected_version,
                approval.kind,
                index == last_index,
                started_at_ms,
                CommandAttemptOutcome::Executed(outcome),
            )? {
                return Ok(view);
            }
        }
        Err(category_error(FailureCategory::Internal))
    }

    /// Runs `executor` the same as [`Self::run_and_record`], but wraps each
    /// individual `start_validation_command` call in its own
    /// `catch_unwind`/`AssertUnwindSafe` — deliberately per-command rather
    /// than around the whole batch, so a panic is attributed to the exact
    /// command that caused it. This matters here in a way it does not for
    /// `crate::planning_execution`/`crate::implementation_execution`:
    /// `task_validation_command_results` is not one row per task, so a
    /// generic "the batch panicked somewhere" fallback could not attach a
    /// result to the right `kind`. A caught panic is classified identically
    /// to a genuine executor `Err`: `Uncertain`, finalized atomically with
    /// `Testing -> RecoveryRequired`. The panic's payload is never
    /// inspected, formatted, or forwarded anywhere — not into this method's
    /// `Result`, not to a log, not to the database — only the
    /// caught/not-caught distinction is used. If the finalize write itself
    /// fails, this returns that failure without retrying or treating it as
    /// success; the task may be left in `Testing`.
    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        approvals: &[ValidationCommandApprovalRecord],
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ValidationCommandExecutor,
    {
        if approvals.is_empty() {
            return Err(category_error(FailureCategory::NotFound));
        }
        let last_index = approvals.len() - 1;
        for (index, approval) in approvals.iter().enumerate() {
            let started_at_ms = self.now_ms()?;
            let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                executor.start_validation_command(Path::new(worktree_path), approval, cancellation)
            }));
            let attempt = match panic_result {
                Ok(outcome) => CommandAttemptOutcome::Executed(outcome),
                Err(_) => CommandAttemptOutcome::Panicked,
            };
            if let Some(view) = self.process_attempt(
                task_id,
                expected_version,
                approval.kind,
                index == last_index,
                started_at_ms,
                attempt,
            )? {
                return Ok(view);
            }
        }
        Err(category_error(FailureCategory::Internal))
    }

    /// Classifies one command attempt and either appends it as an
    /// intermediate `Success` (`Ok(None)`, batch continues) or ends the
    /// batch by finalizing it atomically with the resulting state
    /// transition (`Ok(Some(view))`). A pre-spawn binding rejection returns
    /// `Err` directly: no result is appended and no state changes, so the
    /// caller can retry the same `Testing` task once the approval is fixed.
    fn process_attempt(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        kind: ValidationCommandKind,
        is_last: bool,
        started_at_ms: i64,
        attempt: CommandAttemptOutcome,
    ) -> Result<Option<TaskView>, ApplicationError> {
        let (result_outcome, exit_code) = match attempt {
            CommandAttemptOutcome::Executed(Ok(
                ValidationExecutionStartOutcome::BindingRejected(_rejection),
            )) => {
                return Err(category_error(FailureCategory::InvariantViolation));
            }
            CommandAttemptOutcome::Executed(Ok(ValidationExecutionStartOutcome::Completed(
                outcome,
            ))) => classify_execution_outcome(outcome),
            CommandAttemptOutcome::Executed(Err(_)) | CommandAttemptOutcome::Panicked => {
                (ValidationCommandResultOutcome::Uncertain, None)
            }
        };
        let completed_at_ms = self.now_ms()?;
        let safe_summary = safe_summary_for(result_outcome);

        if result_outcome == ValidationCommandResultOutcome::Success && !is_last {
            self.repository
                .append_validation_command_result(&ValidationCommandResultAttempt {
                    task_id,
                    approved_task_version: expected_version,
                    kind,
                    outcome: result_outcome,
                    exit_code,
                    safe_summary,
                    started_at_ms,
                    completed_at_ms,
                })
                .map_err(|error| ApplicationError::from_categorized(&error))?;
            return Ok(None);
        }

        TaskService::new(self.repository, self.time)
            .finalize_validation_command_batch(FinalizeValidationCommandBatchRequest::new(
                task_id,
                expected_version,
                kind,
                result_outcome,
                exit_code,
                safe_summary,
                started_at_ms,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ))
            .map(Some)
    }

    fn now_ms(&mut self) -> Result<i64, ApplicationError> {
        self.time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))
    }
}

/// Reduces an executor's completed outcome to the safe storage vocabulary
/// plus the exit code that vocabulary allows (`Some` only for `Success`/
/// `ExitFailure`, matching `task_validation_command_results`' own `CHECK`).
fn classify_execution_outcome(
    outcome: ValidationExecutionOutcome,
) -> (ValidationCommandResultOutcome, Option<i32>) {
    match outcome {
        ValidationExecutionOutcome::Success => (ValidationCommandResultOutcome::Success, Some(0)),
        ValidationExecutionOutcome::ExitFailure { exit_code } => {
            (ValidationCommandResultOutcome::ExitFailure, Some(exit_code))
        }
        ValidationExecutionOutcome::TimedOut => (ValidationCommandResultOutcome::TimedOut, None),
        ValidationExecutionOutcome::StdoutBoundExceeded => {
            (ValidationCommandResultOutcome::StdoutBoundExceeded, None)
        }
        ValidationExecutionOutcome::Cancelled => (ValidationCommandResultOutcome::Cancelled, None),
        ValidationExecutionOutcome::Uncertain => (ValidationCommandResultOutcome::Uncertain, None),
    }
}

/// Fixed, outcome-only safe text — never derived from raw stdout/stderr,
/// which this orchestration layer never receives from the executor in the
/// first place (see `chatoms_ports::validation_execution::
/// ValidationCommandExecutor`'s own contract).
fn safe_summary_for(outcome: ValidationCommandResultOutcome) -> String {
    match outcome {
        ValidationCommandResultOutcome::Success => "validation command completed successfully",
        ValidationCommandResultOutcome::ExitFailure => {
            "validation command exited with a nonzero status"
        }
        ValidationCommandResultOutcome::TimedOut => "validation command exceeded its time limit",
        ValidationCommandResultOutcome::StdoutBoundExceeded => {
            "validation command output exceeded the allowed size"
        }
        ValidationCommandResultOutcome::Cancelled => "validation command was cancelled",
        ValidationCommandResultOutcome::Uncertain => {
            "validation command outcome could not be confirmed"
        }
    }
    .to_owned()
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
