//! Shared read-only preflight for a task's `MergeConflict` manual
//! resolution, plus the use case that turns a passing preflight into an
//! immutable confirmation row. [`verify_preconditions`] is deliberately
//! exported at `pub(crate)` visibility so `crate::merge_continue`'s starter
//! can re-run the *exact same* checks fresh immediately before committing
//! `MergeConflict -> Merging` — the point of running this twice (once here,
//! once there) is that state can change in between; neither call trusts the
//! other's result.

use std::path::Path;

use chatoms_domain::{Task, TaskId, TaskState, ValidationCommandKind, ValidationExecutionScope};
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    filesystem::{DirectoryIdentity, FilesystemIdentityPort},
    git::RepositoryKind,
    manual_merge_resolution::{
        ManualMergeResolutionCandidatePort, ManualMergeResolutionCandidateRequest,
        ManualResolutionCandidate, ManualResolutionCandidateOutcome,
    },
    repository::{FoundationRepository, GitIsolationStatus, RepositoryError},
};

use crate::{
    error::ApplicationError,
    tasks::{
        ManualMergeResolutionConfirmationView, RecordManualMergeResolutionConfirmationRequest,
        TaskService,
    },
};

/// Everything a passing preflight establishes: content-free identity a
/// caller needs to either record a confirmation or assemble a
/// `chatoms_ports::merge_continue::MergeContinueRequest`. Never carries raw
/// path/content beyond what [`DirectoryIdentity`]/[`ManualResolutionCandidate`]
/// already expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualMergeResolutionPreflight {
    pub task: Task,
    pub source_approval_task_version: u64,
    pub original_checkout: DirectoryIdentity,
    pub original_common_dir: DirectoryIdentity,
    pub task_worktree: DirectoryIdentity,
    pub base_branch: String,
    pub base_commit: String,
    pub candidate: ManualResolutionCandidate,
}

/// Runs every read-only precondition a manual-resolution confirmation or a
/// `MergeConflict -> Merging` continuation requires, in the fixed order the
/// Unit's design specifies: exact `MergeConflict` state/version and active
/// lease, the immutable `AwaitingUserDiffApproval -> Merging ->
/// MergeConflict` history chain, the source version's exact diff approval,
/// the source version's `ProjectRoot` `Test`/`Build` approvals, the
/// isolation/project/project-identity record, live filesystem identity for
/// all three directories, and finally a `Ready` manual-resolution
/// candidate. Never writes anything.
pub(crate) fn verify_preconditions<R, F, C>(
    repository: &mut R,
    filesystem: &mut F,
    candidate_port: &mut C,
    task_id: TaskId,
    expected_version: u64,
) -> Result<ManualMergeResolutionPreflight, ApplicationError>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
    C: ManualMergeResolutionCandidatePort,
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
        crate::merge_provenance::resolve_merge_conflict_approval_version(&transitions, &task)
    else {
        return Err(category_error(FailureCategory::InvariantViolation));
    };

    let diff_approval = repository
        .get_diff_approval_for_task_version(task_id, source_approval_task_version)
        .map_err(repository_error)?;
    if diff_approval.is_none() {
        return Err(category_error(FailureCategory::NotFound));
    }

    let approvals = repository
        .list_validation_command_approvals_for_scope(
            task_id,
            source_approval_task_version,
            ValidationExecutionScope::ProjectRoot,
        )
        .map_err(repository_error)?;
    let test_approved = approvals
        .iter()
        .any(|approval| approval.kind == ValidationCommandKind::Test);
    let build_approved = approvals
        .iter()
        .any(|approval| approval.kind == ValidationCommandKind::Build);
    if !test_approved || !build_approved {
        return Err(category_error(FailureCategory::NotFound));
    }

    let isolation = repository
        .get_task_isolation(task_id)
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
    if isolation.task_id != task_id
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

    let candidate_request = ManualMergeResolutionCandidateRequest {
        original_checkout: original_checkout.clone(),
        original_common_dir: original_common_dir.clone(),
        task_worktree: task_worktree.clone(),
        task_id,
        project_id: task.project_id(),
        merge_conflict_task_version: expected_version,
        source_approval_task_version,
        task_branch: task.task_branch_identity().as_str().to_owned(),
        base_branch: base_branch.clone(),
        base_commit: base_commit.clone(),
    };
    let candidate = match candidate_port.resolution_candidate(&candidate_request) {
        ManualResolutionCandidateOutcome::Ready(candidate) => candidate,
        ManualResolutionCandidateOutcome::Unresolved
        | ManualResolutionCandidateOutcome::Inconsistent
        | ManualResolutionCandidateOutcome::Unavailable => {
            return Err(category_error(FailureCategory::Conflict));
        }
    };

    Ok(ManualMergeResolutionPreflight {
        task,
        source_approval_task_version,
        original_checkout,
        original_common_dir,
        task_worktree,
        base_branch,
        base_commit,
        candidate,
    })
}

pub struct ConfirmManualMergeResolutionRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl ConfirmManualMergeResolutionRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Records (creates or reuses) an immutable confirmation for whatever the
/// *current* manual-resolution candidate digest is — this service never
/// accepts a caller-supplied digest to compare against, since no per-file
/// diff is shown to the user in this Unit's scope; the confirmation always
/// binds to whatever is staged right now. Never touches task state,
/// transition history, or the `ActiveTaskLease`.
pub struct ManualMergeResolutionConfirmationService<'a, R, T, F, C> {
    repository: &'a mut R,
    time: &'a mut T,
    filesystem: &'a mut F,
    candidate: &'a mut C,
}

impl<'a, R, T, F, C> ManualMergeResolutionConfirmationService<'a, R, T, F, C>
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

    pub fn confirm(
        &mut self,
        request: ConfirmManualMergeResolutionRequest,
    ) -> Result<ManualMergeResolutionConfirmationView, ApplicationError> {
        let preflight = verify_preconditions(
            self.repository,
            self.filesystem,
            self.candidate,
            request.task_id,
            request.expected_version,
        )?;
        let confirmed_at_ms = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        TaskService::new(self.repository, self.time).record_manual_merge_resolution_confirmation(
            RecordManualMergeResolutionConfirmationRequest::new(
                request.task_id,
                request.expected_version,
                preflight.source_approval_task_version,
                preflight.candidate.base_commit.clone(),
                preflight.candidate.task_commit.clone(),
                preflight.candidate.merge_head_commit.clone(),
                preflight.candidate.resolution_digest,
                confirmed_at_ms,
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
