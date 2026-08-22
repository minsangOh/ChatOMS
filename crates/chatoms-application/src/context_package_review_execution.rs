//! Orchestrates a single Context Package v1 Claude Review attempt end to
//! end, reusing every read-only precondition
//! [`crate::review_execution::ReviewExecutionStarter::begin`] already
//! validates (task state/version, a `WorktreeReady` isolation record, the
//! `TaskBrief`, and a usable ephemeral Git diff via
//! [`crate::review_diff::ReviewDiffReader`]) plus one more this Unit adds —
//! that the exact `(task_id, Claude, Review, expected_version,
//! ContextPackageV1)` consent and its FK-bound manifest already exist (see
//! `TaskService::get_context_package_review_readiness`). Neither
//! `crate::review_execution` nor `crate::review_diff` is modified by this
//! module.
//!
//! Unlike Planning/Implementation's Context Package v1 activation, starting
//! Context Package v1 Review drives **no state transition and writes
//! nothing**: `Reviewing` stays `Reviewing`, exactly like the legacy path
//! (see [`crate::review_execution`]'s own doc comment). The
//! `(Claude, Review, expected_version, ContextPackageV1)` consent was
//! already recorded by the separate "Prepare" step
//! (`TaskService::prepare_review_context_package`), so — unlike the legacy
//! path's `TaskService::start_review`, which creates-or-reuses a
//! `LegacyPhase4` consent — this Starter never calls
//! `FoundationRepository::save_review_consent` and there is no
//! `save_context_package_review_transition` repository method to add: there
//! is no transition to commit, so there is nothing for such a method to do.
//! [`ContextPackageReviewExecutionStarter::begin`] is therefore a pure
//! read-only gate, in this fixed order: fresh Claude capability, task
//! state/version, isolation, `TaskBrief`, Context Package v1 readiness, and
//! — last, immediately before a caller would spawn a provider — the live
//! worktree diff read. A diff read that reports
//! [`chatoms_ports::diff::WorktreeDiffOutcome::NoChanges`], `DiffTooLarge`,
//! `TimedOut`, or `Uncertain`, or that itself errors (an identity mismatch,
//! a genuine Git failure, malformed output, ...), never lets a caller reach
//! the executor: no subprocess is ever started, and the task's state,
//! version, history, and lease are left exactly as they were. The diff is
//! read last, not first, to minimize the window between reading a value
//! that changes over time and handing it to the executor — every earlier
//! check is either immutable once true (the `ContextPackageV1`
//! consent/manifest pair) or itself re-verified by
//! [`crate::review_diff::ReviewDiffReader`] just before the actual Git
//! spawn.
//!
//! Split into two services, [`ContextPackageReviewExecutionStarter`] and
//! [`ContextPackageReviewExecutionRecorder`], mirroring every other
//! Context Package v1 execution module's split: the precondition gate is
//! fast and DB-transactional while the actual provider run can take a long
//! time, and both run against the same shared `ReviewRunRegistry`
//! cancellation handle a caller (the Tauri command layer) registers between
//! the two calls.

use std::path::Path;

use chatoms_domain::{TaskId, TaskState};
use chatoms_ports::{
    TimeProvider,
    context_package_review::ContextPackageReviewExecutor,
    diff::{WorktreeDiffOutcome, WorktreeDiffPort},
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    git::GitService,
    process::CancellationSignal,
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::{FoundationRepository, GitIsolationStatus, ReviewResultOutcome, TaskBriefRecord},
    review::{ReviewExecutionBrief, ReviewExecutionStartOutcome},
};

use crate::{
    error::ApplicationError,
    review_diff::{ReadWorktreeDiffRequest, ReviewDiffReader},
    tasks::{RecordReviewResultRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "user";
const RESULT_REASON: &str = "task.review.result";

pub struct BeginContextPackageReviewExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginContextPackageReviewExecutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Everything a caller needs to actually run and then record a Context
/// Package v1 Claude Review attempt, once
/// [`ContextPackageReviewExecutionStarter::begin`] has confirmed every
/// precondition. `task` is unchanged from the request's `expected_version`
/// — like the legacy path, starting Review drives no state transition.
/// `diff_text` is the bounded ephemeral diff `begin` already read; this
/// type's `Debug` output deliberately hides its content (only a byte count
/// is shown), mirroring [`crate::review_execution::ReviewExecutionInputs`]'s
/// own protection.
#[derive(Clone, Eq, PartialEq)]
pub struct ContextPackageReviewExecutionInputs {
    pub task: TaskView,
    pub worktree_path: String,
    pub brief: TaskBriefRecord,
    pub diff_text: String,
}

impl std::fmt::Debug for ContextPackageReviewExecutionInputs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContextPackageReviewExecutionInputs")
            .field("task", &self.task)
            .field("worktree_path", &self.worktree_path)
            .field("brief", &self.brief)
            .field("diff_text_byte_len", &self.diff_text.len())
            .finish()
    }
}

pub struct ContextPackageReviewExecutionStarter<'a, R, T, C, G, F, D> {
    repository: &'a mut R,
    time: &'a mut T,
    capability: &'a mut C,
    git: &'a mut G,
    filesystem: &'a mut F,
    diff: &'a mut D,
}

impl<'a, R, T, C, G, F, D> ContextPackageReviewExecutionStarter<'a, R, T, C, G, F, D>
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
    /// precondition a Context Package v1 Review run needs — task
    /// state/version, a `WorktreeReady` isolation record, a `TaskBrief`, the
    /// exact `(task_id, Claude, Review, expected_version, ContextPackageV1)`
    /// consent/manifest pair, and — last — a usable ephemeral diff
    /// (delegating identity/Git/diff-port verification to
    /// [`ReviewDiffReader`]). On an unsupported capability, a non-`Reviewing`
    /// state, a stale version, missing isolation/brief evidence, a
    /// missing/partial Context Package v1 preparation, or a diff that is
    /// empty/oversized/unconfirmed, nothing is written and the task's state,
    /// version, history, and lease are left exactly as they were: no
    /// consent is ever recorded here (it was already recorded by the
    /// separate "Prepare" step), and no subprocess is ever spawned.
    pub fn begin(
        &mut self,
        request: BeginContextPackageReviewExecutionRequest,
    ) -> Result<ContextPackageReviewExecutionInputs, ApplicationError> {
        let capabilities = self
            .capability
            .provider_capabilities()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Err(category_error(FailureCategory::Unsupported));
        }

        let (task, worktree_path, brief) =
            self.load_execution_evidence(request.task_id, request.expected_version)?;

        let readiness = TaskService::new(self.repository, self.time)
            .get_context_package_review_readiness(request.task_id, request.expected_version)?;
        if !readiness.ready {
            return Err(category_error(FailureCategory::InvalidState));
        }

        let diff_text = self.read_diff(request.task_id, request.expected_version)?;

        Ok(ContextPackageReviewExecutionInputs {
            task,
            worktree_path,
            brief,
            diff_text,
        })
    }

    /// Loads and validates the read-only evidence a Review run requires,
    /// without writing anything: task state/version, a `WorktreeReady`
    /// isolation record, and the `TaskBrief`. Deliberately duplicates the
    /// shape of `crate::review_execution::ReviewExecutionStarter`'s private
    /// `load_execution_evidence` rather than sharing it (that method is
    /// private to `crate::review_execution`, which this Unit does not
    /// modify), but also returns the loaded `TaskView` since — unlike the
    /// legacy path — no later `TaskService::start_review` call will supply
    /// one.
    fn load_execution_evidence(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<(TaskView, String, TaskBriefRecord), ApplicationError> {
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

        Ok((TaskView::from(&task), worktree_path, brief))
    }

    /// Reads the current worktree diff via [`ReviewDiffReader`] (which
    /// itself re-verifies task state/version, isolation, and Git/filesystem
    /// identity before ever spawning a Git process), and rejects every
    /// outcome except a non-empty, in-bound
    /// [`WorktreeDiffOutcome::Diff`] as a typed, state-preserving error. No
    /// subprocess for Claude Review may ever be spawned on the back of an
    /// empty, oversized, or unconfirmed diff. Identical to
    /// `crate::review_execution::ReviewExecutionStarter`'s private
    /// `read_diff`.
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

pub struct ContextPackageReviewExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> ContextPackageReviewExecutionRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    /// Runs `executor` for the Context Package v1 Review attempt already
    /// validated by [`ContextPackageReviewExecutionStarter::begin`], then
    /// records its outcome via the exact same, unmodified
    /// `TaskService::record_review_result` the legacy path uses, atomically
    /// with the resulting state transition. A preflight rejection or a
    /// genuine executor failure both fall back to `RecoveryRequired`, the
    /// same fallback `TaskService` already uses when its own atomic result
    /// write fails.
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
        X: ContextPackageReviewExecutor,
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
    /// panic anywhere in that call the same way every other Context Package
    /// v1/legacy `*Recorder::run_and_record_with_panic_containment` does:
    /// the panic's payload is never inspected, formatted, or forwarded
    /// anywhere. On a caught panic, this attempts the exact same
    /// `RecoveryRequired` fallback [`Self::run_and_record`] already uses for
    /// a non-panicking failure. If the fallback write itself fails, this
    /// returns that failure without retrying or treating it as success; the
    /// task may be left in `Reviewing`.
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
        X: ContextPackageReviewExecutor,
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
