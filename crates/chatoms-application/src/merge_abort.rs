//! Read-only preflight for aborting a task's `MergeConflict` merge, plus the
//! application services that turn a passing preflight into an immutable
//! approval and then, given an existing approval, execute the abort and
//! record its outcome.
//!
//! Deliberately does not reuse
//! [`crate::manual_merge_resolution::verify_preconditions`]: that function
//! requires a `Ready` manual-resolution candidate (unmerged count == 0),
//! but the primary abort use case is an *unresolved* conflict. It also does
//! not require a diff approval or `ProjectRoot` `Test`/`Build` validation
//! approvals — those authorize creating a merge commit and running
//! post-merge validation, neither of which an abort does, and requiring
//! them here would let a missing or corrupted approval row block the only
//! safe exit from a stuck `MergeConflict`.
//!
//! Unlike [`crate::merge_continue`], there is no `MergeAbortStarter`: abort
//! approval never commits a state transition (see
//! [`chatoms_ports::repository::FoundationRepository::ensure_merge_abort_approval`]),
//! so there is no pre-write transition for a starter to own. Assembling a
//! [`chatoms_ports::merge_abort::MergeAbortRequest`] from a fresh preflight
//! and an existing approval is left to the caller -- [`verify_abort_preconditions`]
//! is `pub` (rather than `pub(crate)`) precisely so the Tauri orchestration
//! layer (`src-tauri::commands::merge_abort`) can call it directly to obtain
//! the directory identities and branch names a [`MergeAbortRequest`] needs,
//! immediately before spawning the background write. This is a visibility
//! widening only: the function's preconditions, return type, and fail-closed
//! behavior are unchanged from Phase 5e-3.

use std::path::Path;

use chatoms_domain::{Task, TaskId, TaskState};
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    filesystem::{DirectoryIdentity, FilesystemIdentityPort},
    git::{GitService, RepositoryKind},
    merge_abort::{MergeAbortOutcome, MergeAbortPort, MergeAbortRequest},
    repository::{FoundationRepository, GitIsolationStatus, RepositoryError},
};

use crate::{
    error::ApplicationError,
    tasks::{
        MergeAbortApprovalView, RecordMergeAbortApprovalRequest, RecordMergeAbortResultRequest,
        TaskService, TaskView,
    },
};

const ACTOR_KIND: &str = "user";
const RESULT_REASON: &str = "task.merge-abort.result";

/// Everything a passing preflight establishes: content-free identity a
/// caller needs to either record an approval or assemble a
/// [`MergeAbortRequest`]. Never carries raw path/content beyond what
/// [`DirectoryIdentity`] already exposes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeAbortPreflight {
    pub task: Task,
    pub source_approval_task_version: u64,
    pub original_checkout: DirectoryIdentity,
    pub original_common_dir: DirectoryIdentity,
    pub task_worktree: DirectoryIdentity,
    pub base_branch: String,
    pub base_commit: String,
}

/// Runs every read-only precondition a merge-abort approval requires: exact
/// `MergeConflict` state/version and active lease, the immutable
/// `AwaitingUserDiffApproval -> Merging -> MergeConflict` history chain, and
/// the isolation/project/project-identity record plus live filesystem
/// identity for all three directories. Deliberately does **not** require a
/// diff approval, `ProjectRoot` `Test`/`Build` approvals, or a `Ready`
/// manual-resolution candidate — see this module's top-level documentation.
/// Never writes anything.
pub fn verify_abort_preconditions<R, F>(
    repository: &mut R,
    filesystem: &mut F,
    task_id: TaskId,
    expected_version: u64,
) -> Result<MergeAbortPreflight, ApplicationError>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
{
    let task = repository
        .get_task(task_id)
        .map_err(repository_error)?
        .ok_or_else(|| category_error(FailureCategory::NotFound))?;
    if task.version() != expected_version {
        return Err(category_error(FailureCategory::VersionConflict));
    }
    if task.state() != TaskState::MergeConflict {
        return Err(category_error(FailureCategory::InvalidState));
    }

    let lease = repository.active_lease().map_err(repository_error)?;
    if lease.map(|active| active.task_id) != Some(task_id) {
        return Err(category_error(FailureCategory::InvariantViolation));
    }

    let transitions = repository
        .list_task_transitions(task_id)
        .map_err(repository_error)?;
    let Some(source_approval_task_version) =
        crate::merge_conflict_inspection::merge_chain_approval_version(&transitions, &task)
    else {
        return Err(category_error(FailureCategory::InvariantViolation));
    };

    let isolation = repository
        .get_task_isolation(task_id)
        .map_err(repository_error)?
        .ok_or_else(|| category_error(FailureCategory::NotFound))?;
    if isolation.task_id != task_id
        || isolation.project_id != task.project_id()
        || isolation.status != GitIsolationStatus::WorktreeReady
        || isolation.expected_task_version != expected_version
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

    let project = repository
        .get_project(task.project_id())
        .map_err(repository_error)?
        .ok_or_else(|| category_error(FailureCategory::NotFound))?;
    let identity = repository
        .get_project_identity(task.project_id())
        .map_err(repository_error)?
        .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
    let (Some(expected_common_volume), Some(expected_common_file)) = (
        identity.git_common_volume_serial_hex.as_deref(),
        identity.git_common_file_id_hex.as_deref(),
    ) else {
        return Err(category_error(FailureCategory::InvariantViolation));
    };
    if !identity.confirmed || identity.repository_kind != RepositoryKind::Git {
        return Err(category_error(FailureCategory::InvariantViolation));
    }

    let original_checkout = filesystem
        .inspect_supported_directory(Path::new(&project.root_path))
        .map_err(port_error)?;
    let original_common_dir = filesystem
        .inspect_supported_directory(&original_checkout.canonical_path.join(".git"))
        .map_err(port_error)?;
    let task_worktree = filesystem
        .inspect_supported_directory(Path::new(&worktree_path))
        .map_err(port_error)?;
    if original_checkout.canonical_path != Path::new(&project.root_path)
        || original_checkout.volume_serial_hex != identity.root_volume_serial_hex
        || original_checkout.file_id_hex != identity.root_file_id_hex
        || original_common_dir.canonical_path != original_checkout.canonical_path.join(".git")
        || original_common_dir.volume_serial_hex != expected_common_volume
        || original_common_dir.file_id_hex != expected_common_file
        || task_worktree.canonical_path != Path::new(&worktree_path)
    {
        return Err(category_error(FailureCategory::InvariantViolation));
    }

    Ok(MergeAbortPreflight {
        task,
        source_approval_task_version,
        original_checkout,
        original_common_dir,
        task_worktree,
        base_branch,
        base_commit,
    })
}

pub struct ApproveMergeAbortRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl ApproveMergeAbortRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Records (creates or reuses) an immutable merge-abort approval for the
/// current task worktree HEAD. `task_commit_hex` and `merge_head_hex` are
/// always the same value in `task_merge_abort_approvals` (see migration
/// `0022`'s `CHECK (task_commit_hex = merge_head_hex)`) because the task
/// worktree's own HEAD *is* the commit a `MergeConflict` merges into the
/// base branch — this service reads it once via
/// [`GitService::repository_status`] and uses it for both fields, rather
/// than requiring a separate live `MERGE_HEAD` read. Whether that value
/// still matches the *original checkout*'s live `MERGE_HEAD` at execution
/// time is the executor's job, re-verified fresh immediately before any
/// write (see `chatoms_infrastructure::merge_abort`) — this service never
/// touches task state, transition history, or the `ActiveTaskLease`.
pub struct MergeAbortApprovalService<'a, R, T, F, G> {
    repository: &'a mut R,
    time: &'a mut T,
    filesystem: &'a mut F,
    git: &'a mut G,
}

impl<'a, R, T, F, G> MergeAbortApprovalService<'a, R, T, F, G>
where
    R: FoundationRepository,
    T: TimeProvider,
    F: FilesystemIdentityPort,
    G: GitService,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        time: &'a mut T,
        filesystem: &'a mut F,
        git: &'a mut G,
    ) -> Self {
        Self {
            repository,
            time,
            filesystem,
            git,
        }
    }

    pub fn approve(
        &mut self,
        request: ApproveMergeAbortRequest,
    ) -> Result<MergeAbortApprovalView, ApplicationError> {
        let preflight = verify_abort_preconditions(
            self.repository,
            self.filesystem,
            request.task_id,
            request.expected_version,
        )?;
        let status = self
            .git
            .repository_status(&preflight.task_worktree.canonical_path)
            .map_err(port_error)?;
        let task_branch = preflight.task.task_branch_identity().as_str();
        let Some(task_commit) = status.head_commit.filter(|_| {
            status.current_branch.as_deref() == Some(task_branch) && !status.detached_head
        }) else {
            return Err(category_error(FailureCategory::Conflict));
        };
        let approved_at_ms = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        TaskService::new(self.repository, self.time).record_merge_abort_approval(
            RecordMergeAbortApprovalRequest::new(
                request.task_id,
                request.expected_version,
                preflight.source_approval_task_version,
                preflight.base_commit.clone(),
                task_commit.clone(),
                task_commit,
                approved_at_ms,
            ),
        )
    }
}

pub struct MergeAbortRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> MergeAbortRecorder<'a, R, T>
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
        request: &MergeAbortRequest,
        executor: &mut X,
    ) -> Result<TaskView, ApplicationError>
    where
        X: MergeAbortPort,
    {
        let outcome = executor.abort_merge(request);
        self.record(task_id, expected_version, outcome)
    }

    /// No cancellation is supported for this write (a short `merge --abort`
    /// is never interrupted mid-flight), but the executor call is still
    /// wrapped in panic containment for parity with every other Git-write
    /// orchestration in this crate. A caught panic is treated the same as
    /// [`MergeAbortOutcome::PostWriteUncertain`] — the task remains
    /// `MergeConflict` and a subsequent abort attempt can recover via the
    /// `ConfirmedNotInMerge` path.
    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        request: &MergeAbortRequest,
        executor: &mut X,
    ) -> Result<TaskView, ApplicationError>
    where
        X: MergeAbortPort,
    {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_and_record(task_id, expected_version, request, executor)
        })) {
            Ok(result) => result,
            Err(_) => self.record(
                task_id,
                expected_version,
                MergeAbortOutcome::PostWriteUncertain,
            ),
        }
    }

    fn record(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        outcome: MergeAbortOutcome,
    ) -> Result<TaskView, ApplicationError> {
        TaskService::new(self.repository, self.time).record_merge_abort_result(
            RecordMergeAbortResultRequest::new(
                task_id,
                expected_version,
                outcome,
                ACTOR_KIND.to_owned(),
                RESULT_REASON.to_owned(),
            ),
        )
    }
}

fn repository_error(error: RepositoryError) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn port_error(error: chatoms_ports::error::PortFailure) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
