//! Orchestrates a single Claude Review attempt end to end: revalidating
//! every read-only precondition (task state/version, a `WorktreeReady`
//! isolation record, the `TaskBrief`, and a usable ephemeral Git diff via
//! `crate::review_diff::ReviewDiffReader`) *before* committing anything,
//! then — once every precondition is confirmed — recording or reusing a
//! same-version Claude/Review consent (delegating to
//! `TaskService::start_review`, which itself drives no state transition:
//! `Reviewing` stays `Reviewing`), and — once a caller has actually run a
//! `ClaudeReviewExecutor` — recording its outcome (delegating to
//! `TaskService::record_review_result`).
//!
//! Split into two services, [`ReviewExecutionStarter`] and
//! [`ReviewExecutionRecorder`], mirroring `crate::planning_execution`'s and
//! `crate::implementation_execution`'s split: the precondition-and-consent
//! step is fast and DB-transactional while the actual provider run can take
//! a long time.
//!
//! Unlike
//! [`crate::planning_execution::PlanningExecutionStarter::begin`] (which
//! commits its transition first and falls back to `RecoveryRequired` only if
//! the follow-up evidence load then fails) and like
//! [`crate::implementation_execution::ImplementationExecutionStarter::begin`]
//! (which validates every required piece of evidence before writing
//! anything), [`ReviewExecutionStarter::begin`] validates every read-only
//! precondition — including a successful, non-empty, in-bound diff read —
//! *before* ever calling `TaskService::start_review`. A diff read that
//! reports [`chatoms_ports::diff::WorktreeDiffOutcome::NoChanges`],
//! `DiffTooLarge`, `TimedOut`, or `Uncertain`, or that itself errors (an
//! identity mismatch, a genuine Git failure, malformed output, ...), never
//! reaches `start_review`: no consent is recorded, no subprocess for Claude
//! Review is ever started, and the task's state, version, history, and
//! lease are left exactly as they were.

use std::path::Path;

use chatoms_domain::{TaskId, TaskState};
use chatoms_ports::{
    TimeProvider,
    diff::{WorktreeDiffOutcome, WorktreeDiffPort},
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    git::GitService,
    process::CancellationSignal,
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::{FoundationRepository, GitIsolationStatus, ReviewResultOutcome, TaskBriefRecord},
    review::{ClaudeReviewExecutor, ReviewExecutionBrief, ReviewExecutionStartOutcome},
};

use crate::{
    error::ApplicationError,
    review_diff::{ReadWorktreeDiffRequest, ReviewDiffReader},
    tasks::{RecordReviewResultRequest, StartReviewRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "user";
const RESULT_REASON: &str = "task.review.result";

pub struct BeginReviewExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginReviewExecutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything a caller needs to actually run and then record a Claude
/// Review attempt, once [`ReviewExecutionStarter::begin`] has confirmed
/// every precondition and recorded/reused the Claude/Review consent.
/// `task` is unchanged from the request's `expected_version` — unlike
/// Planning/Implementation, starting Review drives no state transition.
/// `diff_text` is the bounded ephemeral diff `begin` already read; this
/// type's `Debug` output deliberately hides its content (only a byte count
/// is shown), mirroring [`chatoms_ports::diff::WorktreeDiff`]'s own
/// protection, so a stray `{:?}` cannot leak diff content into a log.
#[derive(Clone, Eq, PartialEq)]
pub struct ReviewExecutionInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub brief: TaskBriefRecord,
    pub diff_text: String,
}

impl std::fmt::Debug for ReviewExecutionInputs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReviewExecutionInputs")
            .field("task", &self.task)
            .field("worktree_path", &self.worktree_path)
            .field("brief", &self.brief)
            .field("diff_text_byte_len", &self.diff_text.len())
            .finish()
    }
}

pub struct ReviewExecutionStarter<'a, R, T, C, G, F, D> {
    repository: &'a mut R,
    time: &'a mut T,
    capability: &'a mut C,
    git: &'a mut G,
    filesystem: &'a mut F,
    diff: &'a mut D,
}

impl<'a, R, T, C, G, F, D> ReviewExecutionStarter<'a, R, T, C, G, F, D>
where
    R: FoundationRepository,
    T: TimeProvider,
    C: ProviderCapabilityPort,
    G: GitService,
    F: FilesystemIdentityPort,
    D: WorktreeDiffPort,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        time: &'a mut T,
        capability: &'a mut C,
        git: &'a mut G,
        filesystem: &'a mut F,
        diff: &'a mut D,
    ) -> Self {
        Self {
            repository,
            time,
            capability,
            git,
            filesystem,
            diff,
        }
    }

    /// Fresh-checks Claude capability, then validates every read-only
    /// precondition a Review run needs — task state/version, a
    /// `WorktreeReady` isolation record, a `TaskBrief`, and a usable
    /// ephemeral diff (delegating identity/Git/diff-port verification to
    /// [`ReviewDiffReader`]) — and only once every one of those is confirmed
    /// does it record or reuse the Claude/Review consent via
    /// `TaskService::start_review`. On an unsupported capability, a
    /// non-`Reviewing` state, a stale version, missing isolation/brief
    /// evidence, or a diff that is empty/oversized/unconfirmed, nothing is
    /// written and the task's state, version, history, and lease are left
    /// exactly as they were: "no execution, no consent, state preserved".
    pub fn begin(
        &mut self,
        request: BeginReviewExecutionRequest,
    ) -> Result<ReviewExecutionInputs, ApplicationError> {
        let capabilities = self
            .capability
            .provider_capabilities()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Err(category_error(FailureCategory::Unsupported));
        }

        let (worktree_path, brief) =
            self.load_execution_evidence(request.task_id, request.expected_version)?;
        let diff_text = self.read_diff(request.task_id, request.expected_version)?;

        let task = TaskService::new(self.repository, self.time).start_review(
            StartReviewRequest::new(request.task_id, request.expected_version),
        )?;

        Ok(ReviewExecutionInputs {
            task,
            worktree_path,
            brief,
            diff_text,
        })
    }

    /// Loads and validates the read-only evidence a Review run requires,
    /// without writing anything: task state/version (re-checked here even
    /// though [`ReviewDiffReader::read_current_diff`] and
    /// `TaskService::start_review` each re-check it again independently —
    /// every layer in this call chain re-verifies rather than trusting an
    /// earlier check), a `WorktreeReady` isolation record, and the
    /// `TaskBrief`.
    fn load_execution_evidence(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<(String, TaskBriefRecord), ApplicationError> {
        let task = self
            .repository
            .get_task(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::Reviewing {
            return Err(category_error(FailureCategory::InvalidState));
        }

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

    /// Reads the current worktree diff via [`ReviewDiffReader`] (which
    /// itself re-verifies task state/version, isolation, and Git/filesystem
    /// identity before ever spawning a Git process), and rejects every
    /// outcome except a non-empty, in-bound
    /// [`WorktreeDiffOutcome::Diff`] as a typed, state-preserving error. No
    /// subprocess for Claude Review may ever be spawned, and no consent may
    /// ever be recorded, on the back of an empty, oversized, or unconfirmed
    /// diff.
    fn read_diff(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<String, ApplicationError> {
        let outcome = ReviewDiffReader::new(self.repository, self.git, self.filesystem, self.diff)
            .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, expected_version))?;
        match outcome {
            WorktreeDiffOutcome::Diff(diff) => Ok(diff.text().to_owned()),
            WorktreeDiffOutcome::NoChanges
            | WorktreeDiffOutcome::DiffTooLarge
            | WorktreeDiffOutcome::TimedOut
            | WorktreeDiffOutcome::Uncertain => Err(category_error(FailureCategory::Conflict)),
        }
    }
}

pub struct ReviewExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> ReviewExecutionRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs `executor` for the Review attempt already started by
    /// [`ReviewExecutionStarter::begin`], then records its outcome via
    /// `TaskService::record_review_result` atomically with the resulting
    /// state transition (`Completed -> AwaitingUserDiffApproval`, `Failed ->
    /// Failed`, confirmed `Cancelled -> Paused` with `resume_target_state =
    /// Reviewing`, `RecoveryRequired -> RecoveryRequired`). A post-consent
    /// preflight rejection or a genuine executor failure both fall back to
    /// `RecoveryRequired`, the same fallback `TaskService` already uses when
    /// its own atomic result write fails.
    #[allow(clippy::too_many_arguments)]
    pub fn run_and_record<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        brief: &TaskBriefRecord,
        diff_text: &str,
        started_at_ms: i64,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ClaudeReviewExecutor,
    {
        let outcome = executor.start_review(
            Path::new(worktree_path),
            ReviewExecutionBrief {
                requirements: &brief.requirements,
                completion_criteria: &brief.completion_criteria,
                prohibited_scope: &brief.prohibited_scope,
                diff_text,
            },
            cancellation,
        );

        let (result_outcome, exit_code, turn_count, review_text) = match outcome {
            Ok(ReviewExecutionStartOutcome::Completed(result)) => (
                result.outcome,
                result.exit_code,
                result.turn_count,
                result.review_text,
            ),
            Ok(ReviewExecutionStartOutcome::PreflightRejected) | Err(_) => {
                (ReviewResultOutcome::RecoveryRequired, None, None, None)
            }
        };

        TaskService::new(self.repository, self.time).record_review_result(
            RecordReviewResultRequest::new(
                task_id,
                expected_version,
                result_outcome,
                exit_code,
                turn_count,
                review_text,
                started_at_ms,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ),
        )
    }

    /// Runs `executor` the same as [`Self::run_and_record`], but contains a
    /// panic anywhere in that call the same way
    /// [`crate::planning_execution::PlanningExecutionRecorder::run_and_record_with_panic_containment`]
    /// and
    /// [`crate::implementation_execution::ImplementationExecutionRecorder::run_and_record_with_panic_containment`]
    /// do: the panic's payload is never inspected, formatted, or forwarded
    /// anywhere — not into this method's `Result`, not to a log, not to the
    /// database — only the caught/not-caught distinction is used. On a
    /// caught panic, this attempts the exact same `RecoveryRequired`
    /// fallback [`Self::run_and_record`] already uses for a non-panicking
    /// failure. If the fallback write itself fails, this returns that
    /// failure without retrying or treating it as success; the task may be
    /// left in `Reviewing`.
    #[allow(clippy::too_many_arguments)]
    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        worktree_path: &str,
        brief: &TaskBriefRecord,
        diff_text: &str,
        started_at_ms: i64,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ClaudeReviewExecutor,
    {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_and_record(
                task_id,
                expected_version,
                worktree_path,
                brief,
                diff_text,
                started_at_ms,
                executor,
                cancellation,
            )
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => TaskService::new(self.repository, self.time).record_review_result(
                RecordReviewResultRequest::new(
                    task_id,
                    expected_version,
                    ReviewResultOutcome::RecoveryRequired,
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
