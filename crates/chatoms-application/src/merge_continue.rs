//! Orchestrates `MergeConflict -> Merging -> PostMergeTesting |
//! MergeConflict | RecoveryRequired`: re-verifies every precondition
//! [`crate::manual_merge_resolution::verify_preconditions`] checks, then
//! requires that an exact immutable confirmation already exists for the
//! *live* candidate digest before committing the state transition. Mirrors
//! [`crate::merge_execution::MergeExecutionStarter`]/
//! [`crate::merge_execution::MergeExecutionRecorder`]'s shape.

use chatoms_domain::TaskId;
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    manual_merge_resolution::ManualMergeResolutionCandidatePort,
    merge_continue::{MergeContinueOutcome, MergeContinuePort, MergeContinueRequest},
    repository::FoundationRepository,
};

use crate::{
    error::ApplicationError,
    manual_merge_resolution::verify_preconditions,
    tasks::{RecordMergeContinueResultRequest, StartMergeContinueRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "user";
const START_REASON: &str = "task.merge-continue.start";
const RESULT_REASON: &str = "task.merge-continue.result";

pub struct BeginMergeContinueRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginMergeContinueRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeContinueInputs {
    pub task: TaskView,
    pub request: MergeContinueRequest,
}

pub struct MergeContinueStarter<'a, R, T, F, C> {
    repository: &'a mut R,
    time: &'a mut T,
    filesystem: &'a mut F,
    candidate: &'a mut C,
}

impl<'a, R, T, F, C> MergeContinueStarter<'a, R, T, F, C>
where
    R: FoundationRepository,
    T: TimeProvider,
    F: FilesystemIdentityPort,
    C: ManualMergeResolutionCandidatePort,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        time: &'a mut T,
        filesystem: &'a mut F,
        candidate: &'a mut C,
    ) -> Self {
        Self {
            repository,
            time,
            filesystem,
            candidate,
        }
    }

    /// Re-runs the full read-only preflight, requires that an exact
    /// immutable confirmation already exists for the live candidate
    /// digest, and only then commits `MergeConflict -> Merging` — which
    /// itself re-verifies confirmation existence a second time inside its
    /// own repository transaction (see
    /// `chatoms_ports::repository::FoundationRepository::save_manual_merge_resolution_transition`).
    /// A duplicate start against the same `expected_version` cannot
    /// succeed twice: the first call's transition changes the task's
    /// version, so a second call with the same `expected_version` fails
    /// closed with a version conflict before any Git write is attempted.
    pub fn begin(
        &mut self,
        request: BeginMergeContinueRequest,
    ) -> Result<MergeContinueInputs, ApplicationError> {
        let preflight = verify_preconditions(
            self.repository,
            self.filesystem,
            self.candidate,
            request.task_id,
            request.expected_version,
        )?;
        let confirmation = self
            .repository
            .get_manual_merge_resolution_confirmation(
                request.task_id,
                request.expected_version,
                preflight.candidate.resolution_digest,
            )
            .map_err(repository_error)?;
        if confirmation.is_none() {
            return Err(category_error(FailureCategory::NotFound));
        }

        let project_id = preflight.task.project_id();
        let task_branch = preflight.task.task_branch_identity().as_str().to_owned();
        let merge_request = MergeContinueRequest {
            original_checkout: preflight.original_checkout,
            original_common_dir: preflight.original_common_dir,
            task_worktree: preflight.task_worktree,
            project_id,
            task_id: request.task_id,
            merge_conflict_task_version: request.expected_version,
            source_approval_task_version: preflight.source_approval_task_version,
            base_branch: preflight.base_branch,
            task_branch,
            base_commit: preflight.base_commit,
            task_commit: preflight.candidate.task_commit,
            merge_head_commit: preflight.candidate.merge_head_commit,
            confirmed_resolution_digest: preflight.candidate.resolution_digest,
        };

        let task = TaskService::new(self.repository, self.time).start_merge_continue(
            StartMergeContinueRequest::new(
                request.task_id,
                request.expected_version,
                preflight.candidate.resolution_digest,
                ACTOR_KIND.to_owned(),
                START_REASON.to_owned(),
            ),
        )?;
        Ok(MergeContinueInputs {
            task,
            request: merge_request,
        })
    }
}

pub struct MergeContinueRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> MergeContinueRecorder<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    pub fn run_and_record<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        request: &MergeContinueRequest,
        executor: &mut X,
    ) -> Result<TaskView, ApplicationError>
    where
        X: MergeContinuePort,
    {
        let outcome = executor.continue_merge(request);
        self.record(task_id, expected_version, outcome)
    }

    /// No cancellation is supported for this write (see
    /// `chatoms_ports::merge_continue`'s documentation), but the executor
    /// call is still wrapped in panic containment for parity with every
    /// other Git-write orchestration in this crate.
    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        request: &MergeContinueRequest,
        executor: &mut X,
    ) -> Result<TaskView, ApplicationError>
    where
        X: MergeContinuePort,
    {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_and_record(task_id, expected_version, request, executor)
        })) {
            Ok(result) => result,
            Err(_) => self.record(
                task_id,
                expected_version,
                MergeContinueOutcome::PostWriteUncertain,
            ),
        }
    }

    fn record(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        outcome: MergeContinueOutcome,
    ) -> Result<TaskView, ApplicationError> {
        TaskService::new(self.repository, self.time).record_merge_continue_result(
            RecordMergeContinueResultRequest::new(
                task_id,
                expected_version,
                outcome,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ),
        )
    }
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}

fn repository_error(error: chatoms_ports::repository::RepositoryError) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}
