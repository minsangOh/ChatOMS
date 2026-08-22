use std::path::Path;

use chatoms_domain::{TaskId, TaskState, ValidationCommandKind, ValidationExecutionScope};
use chatoms_ports::{
    TimeProvider,
    diff::DiffContentHash,
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    merge_execution::{MergeExecutionOutcome, MergeExecutionPort, MergeExecutionRequest},
    repository::{FoundationRepository, GitIsolationStatus},
};

use crate::{
    error::ApplicationError,
    tasks::{RecordMergeResultRequest, StartMergeRequest, TaskService, TaskView},
};

const ACTOR_KIND: &str = "user";
const START_REASON: &str = "task.merge.start";
const RESULT_REASON: &str = "task.merge.result";

pub struct BeginMergeExecutionRequest {
    task_id: TaskId,
    expected_version: u64,
    approved_diff_content_hash: DiffContentHash,
}

impl BeginMergeExecutionRequest {
    #[must_use]
    pub const fn new(
        task_id: TaskId,
        expected_version: u64,
        approved_diff_content_hash: DiffContentHash,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            approved_diff_content_hash,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeExecutionInputs {
    pub task: TaskView,
    pub request: MergeExecutionRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectRootValidationApprovalStatus {
    pub test_approved: bool,
    pub build_approved: bool,
}

impl ProjectRootValidationApprovalStatus {
    #[must_use]
    pub const fn ready(self) -> bool {
        self.test_approved && self.build_approved
    }
}

pub struct MergeExecutionStarter<'a, R, T, F> {
    repository: &'a mut R,
    time: &'a mut T,
    filesystem: &'a mut F,
}

impl<'a, R, T, F> MergeExecutionStarter<'a, R, T, F>
where
    R: FoundationRepository,
    T: TimeProvider,
    F: FilesystemIdentityPort,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T, filesystem: &'a mut F) -> Self {
        Self {
            repository,
            time,
            filesystem,
        }
    }

    pub fn begin(
        &mut self,
        request: BeginMergeExecutionRequest,
    ) -> Result<MergeExecutionInputs, ApplicationError> {
        self.require_project_root_validation_approvals(request.task_id, request.expected_version)?;
        let merge_request = self.load_execution_request(&request)?;
        let task =
            TaskService::new(self.repository, self.time).start_merge(StartMergeRequest::new(
                request.task_id,
                request.expected_version,
                ACTOR_KIND.to_owned(),
                START_REASON.to_owned(),
            ))?;
        Ok(MergeExecutionInputs {
            task,
            request: merge_request,
        })
    }

    /// Reads only content-free ProjectRoot approval presence for this exact
    /// task version. It never substitutes TaskWorktree approvals and never
    /// writes or transitions a task.
    pub fn project_root_validation_approval_status(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<ProjectRootValidationApprovalStatus, ApplicationError> {
        let task = self
            .repository
            .get_task(task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::AwaitingUserDiffApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let approvals = self
            .repository
            .list_validation_command_approvals_for_scope(
                task_id,
                expected_version,
                ValidationExecutionScope::ProjectRoot,
            )
            .map_err(repository_error)?;
        Ok(ProjectRootValidationApprovalStatus {
            test_approved: approvals
                .iter()
                .any(|approval| approval.kind == ValidationCommandKind::Test),
            build_approved: approvals
                .iter()
                .any(|approval| approval.kind == ValidationCommandKind::Build),
        })
    }

    pub fn require_project_root_validation_approvals(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<(), ApplicationError> {
        if self
            .project_root_validation_approval_status(task_id, expected_version)?
            .ready()
        {
            Ok(())
        } else {
            Err(category_error(FailureCategory::NotFound))
        }
    }

    fn load_execution_request(
        &mut self,
        request: &BeginMergeExecutionRequest,
    ) -> Result<MergeExecutionRequest, ApplicationError> {
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::AwaitingUserDiffApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        self.require_project_root_validation_approvals(request.task_id, request.expected_version)?;
        let approval = self
            .repository
            .get_diff_approval(
                request.task_id,
                request.expected_version,
                request.approved_diff_content_hash,
            )
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if approval.task_id != request.task_id
            || approval.approved_task_version != request.expected_version
            || approval.diff_content_hash != request.approved_diff_content_hash
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let isolation = self
            .repository
            .get_task_isolation(request.task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        // `TaskGitIsolation.expected_task_version` is the optimistic-
        // concurrency value of the *isolation* lifecycle: it is stamped when a
        // worktree operation is recorded and frozen once the isolation reaches
        // `WorktreeReady`. It is deliberately not compared against the task's
        // current version here — by the time a task reaches this point it has
        // advanced many versions past `WorktreeReady`, so such a comparison
        // could never hold. The task's own version is verified above; the
        // isolation is verified by identity and status instead.
        if isolation.task_id != request.task_id
            || isolation.project_id != task.project_id()
            || isolation.status != GitIsolationStatus::WorktreeReady
            || !isolation.branch_created_by_app
            || !isolation.worktree_created_by_app
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let (base_branch, base_commit, worktree_path) = match (
            isolation.base_branch,
            isolation.base_commit,
            isolation.worktree_path,
        ) {
            (Some(base_branch), Some(base_commit), Some(worktree_path)) => {
                (base_branch, base_commit, worktree_path)
            }
            _ => return Err(category_error(FailureCategory::InvariantViolation)),
        };
        let project = self
            .repository
            .get_project(task.project_id())
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        let original_checkout = self
            .filesystem
            .inspect_supported_directory(Path::new(&project.root_path))
            .map_err(port_error)?;
        let original_common_dir = self
            .filesystem
            .inspect_supported_directory(&original_checkout.canonical_path.join(".git"))
            .map_err(port_error)?;
        let task_worktree = self
            .filesystem
            .inspect_supported_directory(Path::new(&worktree_path))
            .map_err(port_error)?;
        self.filesystem
            .verify_local_tree(&original_checkout.canonical_path)
            .and_then(|_| {
                self.filesystem
                    .verify_local_tree(&task_worktree.canonical_path)
            })
            .map_err(port_error)?;
        Ok(MergeExecutionRequest {
            original_checkout,
            original_common_dir,
            task_worktree,
            task_branch: task.task_branch_identity().as_str().to_owned(),
            base_branch,
            base_commit,
            approved_diff_content_hash: request.approved_diff_content_hash,
        })
    }
}

pub struct MergeExecutionRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> MergeExecutionRecorder<'a, R, T>
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
        request: &MergeExecutionRequest,
        executor: &mut X,
    ) -> Result<TaskView, ApplicationError>
    where
        X: MergeExecutionPort,
    {
        let outcome = executor.commit_and_merge(request);
        self.record(task_id, expected_version, outcome)
    }

    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        request: &MergeExecutionRequest,
        executor: &mut X,
    ) -> Result<TaskView, ApplicationError>
    where
        X: MergeExecutionPort,
    {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_and_record(task_id, expected_version, request, executor)
        })) {
            Ok(result) => result,
            Err(_) => self.record(
                task_id,
                expected_version,
                MergeExecutionOutcome::PostWriteUncertain,
            ),
        }
    }

    fn record(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        outcome: MergeExecutionOutcome,
    ) -> Result<TaskView, ApplicationError> {
        TaskService::new(self.repository, self.time).record_merge_result(
            RecordMergeResultRequest::new(
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

fn port_error(error: chatoms_ports::error::PortFailure) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}
