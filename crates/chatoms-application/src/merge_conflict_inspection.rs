use std::path::Path;

use chatoms_domain::{TaskId, TaskState, TaskStateTransition};
use chatoms_ports::{
    filesystem::FilesystemIdentityPort,
    git::RepositoryKind,
    merge_conflict_inspection::{
        MergeConflictInspectionOutcome, MergeConflictInspectionPort,
        MergeConflictInspectionRequest, MergeConflictInspectionResult,
    },
    repository::{FoundationRepository, GitIsolationStatus},
};

use crate::error::ApplicationError;

pub struct MergeConflictInspectionService<'a, R, F, G> {
    repository: &'a mut R,
    filesystem: &'a mut F,
    git: &'a mut G,
}

impl<'a, R, F, G> MergeConflictInspectionService<'a, R, F, G>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
    G: MergeConflictInspectionPort,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, filesystem: &'a mut F, git: &'a mut G) -> Self {
        Self {
            repository,
            filesystem,
            git,
        }
    }

    pub fn inspect(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<MergeConflictInspectionResult>, ApplicationError> {
        let Some(task) = self
            .repository
            .get_task(task_id)
            .map_err(repository_error)?
        else {
            return Ok(None);
        };
        if task.state() != TaskState::MergeConflict {
            return Ok(None);
        }
        if !self
            .repository
            .active_lease()
            .map_err(repository_error)?
            .is_some_and(|lease| lease.task_id == task_id)
        {
            return Ok(Some(inconsistent()));
        }
        let Some(approval_version) = self.approval_version(&task)? else {
            return Ok(Some(inconsistent()));
        };
        let Some(approval) = self
            .repository
            .get_diff_approval_for_task_version(task_id, approval_version)
            .map_err(repository_error)?
        else {
            return Ok(Some(inconsistent()));
        };
        if approval.task_id != task_id || approval.approved_task_version != approval_version {
            return Ok(Some(inconsistent()));
        }
        let Some(isolation) = self
            .repository
            .get_task_isolation(task_id)
            .map_err(repository_error)?
        else {
            return Ok(Some(inconsistent()));
        };
        let Some(project) = self
            .repository
            .get_project(task.project_id())
            .map_err(repository_error)?
        else {
            return Ok(Some(inconsistent()));
        };
        let Some(identity) = self
            .repository
            .get_project_identity(task.project_id())
            .map_err(repository_error)?
        else {
            return Ok(Some(inconsistent()));
        };
        let (base_branch, base_commit, worktree_path) = match (
            isolation.base_branch,
            isolation.base_commit,
            isolation.worktree_path,
        ) {
            (Some(branch), Some(commit), Some(path)) => (branch, commit, path),
            _ => return Ok(Some(inconsistent())),
        };
        if isolation.task_id != task_id
            || isolation.project_id != task.project_id()
            || isolation.status != GitIsolationStatus::WorktreeReady
            || isolation.expected_task_version != approval_version
            || !isolation.branch_created_by_app
            || !isolation.worktree_created_by_app
            || identity.repository_kind != RepositoryKind::Git
            || identity.git_common_volume_serial_hex.is_none()
            || identity.git_common_file_id_hex.is_none()
        {
            return Ok(Some(inconsistent()));
        }
        let (Some(expected_common_volume), Some(expected_common_file)) = (
            identity.git_common_volume_serial_hex.as_deref(),
            identity.git_common_file_id_hex.as_deref(),
        ) else {
            return Ok(Some(inconsistent()));
        };
        let Some(original_checkout) = self
            .filesystem
            .inspect_supported_directory(Path::new(&project.root_path))
            .ok()
        else {
            return Ok(Some(unavailable()));
        };
        let Some(original_common_dir) = self
            .filesystem
            .inspect_supported_directory(&original_checkout.canonical_path.join(".git"))
            .ok()
        else {
            return Ok(Some(unavailable()));
        };
        let Some(task_worktree) = self
            .filesystem
            .inspect_supported_directory(Path::new(&worktree_path))
            .ok()
        else {
            return Ok(Some(unavailable()));
        };
        if original_checkout.canonical_path != Path::new(&project.root_path)
            || original_checkout.volume_serial_hex != identity.root_volume_serial_hex
            || original_checkout.file_id_hex != identity.root_file_id_hex
            || original_common_dir.canonical_path != original_checkout.canonical_path.join(".git")
            || original_common_dir.volume_serial_hex != expected_common_volume
            || original_common_dir.file_id_hex != expected_common_file
            || task_worktree.canonical_path != Path::new(&worktree_path)
        {
            return Ok(Some(inconsistent()));
        }
        let request = MergeConflictInspectionRequest {
            original_checkout,
            original_common_dir,
            task_worktree,
            task_branch: task.task_branch_identity().as_str().to_owned(),
            base_branch,
            base_commit,
        };
        Ok(Some(self.git.inspect_merge_conflicts(&request)))
    }

    fn approval_version(
        &mut self,
        task: &chatoms_domain::Task,
    ) -> Result<Option<u64>, ApplicationError> {
        let transitions = self
            .repository
            .list_task_transitions(task.id())
            .map_err(repository_error)?;
        Ok(merge_chain_approval_version(&transitions, task))
    }
}

pub(crate) fn merge_chain_approval_version(
    transitions: &[TaskStateTransition],
    task: &chatoms_domain::Task,
) -> Option<u64> {
    transitions.windows(3).find_map(|chain| {
        let consecutive = chain[0].sequence().checked_add(1) == Some(chain[1].sequence())
            && chain[1].sequence().checked_add(1) == Some(chain[2].sequence())
            && chain[0].task_id() == task.id()
            && chain[1].task_id() == task.id()
            && chain[2].task_id() == task.id()
            && chain[0].task_version().checked_add(1) == Some(chain[1].task_version())
            && chain[1].task_version().checked_add(1) == Some(chain[2].task_version());
        (consecutive
            && chain[0].to_state() == TaskState::AwaitingUserDiffApproval
            && chain[1].from_state() == Some(TaskState::AwaitingUserDiffApproval)
            && chain[1].to_state() == TaskState::Merging
            && chain[2].from_state() == Some(TaskState::Merging)
            && chain[2].to_state() == TaskState::MergeConflict
            && chain[2].task_version() == task.version())
        .then_some(chain[0].task_version())
    })
}

fn repository_error(error: chatoms_ports::repository::RepositoryError) -> ApplicationError {
    ApplicationError::from_categorized(&error)
}

fn result(outcome: MergeConflictInspectionOutcome) -> MergeConflictInspectionResult {
    MergeConflictInspectionResult {
        outcome,
        counts: Default::default(),
    }
}

fn inconsistent() -> MergeConflictInspectionResult {
    result(MergeConflictInspectionOutcome::Inconsistent)
}

fn unavailable() -> MergeConflictInspectionResult {
    result(MergeConflictInspectionOutcome::Unavailable)
}
