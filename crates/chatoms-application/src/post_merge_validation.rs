use std::path::Path;

use chatoms_domain::{TaskId, TaskState, ValidationCommandKind, ValidationExecutionScope};
use chatoms_ports::{
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    repository::{FoundationRepository, ValidationCommandApprovalRecord},
    validation_execution::ValidationExecutionTarget,
};

use crate::{error::ApplicationError, tasks::TaskView};

pub struct BeginPostMergeValidationRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl BeginPostMergeValidationRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostMergeValidationInputs {
    pub task: TaskView,
    pub approval_task_version: u64,
    pub target: ValidationExecutionTarget,
    pub approvals: Vec<ValidationCommandApprovalRecord>,
}

pub struct PostMergeValidationStarter<'a, R, F> {
    repository: &'a mut R,
    filesystem: &'a mut F,
}

impl<'a, R, F> PostMergeValidationStarter<'a, R, F>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, filesystem: &'a mut F) -> Self {
        Self {
            repository,
            filesystem,
        }
    }

    pub fn begin(
        &mut self,
        request: BeginPostMergeValidationRequest,
    ) -> Result<PostMergeValidationInputs, ApplicationError> {
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::PostMergeTesting {
            return Err(category_error(FailureCategory::InvalidState));
        }

        let transitions = self
            .repository
            .list_task_transitions(request.task_id)
            .map_err(repository_error)?;
        let Some(approval_task_version) =
            crate::merge_provenance::resolve_post_merge_approval_version(&transitions, &task)
        else {
            return Err(category_error(FailureCategory::InvariantViolation));
        };
        let approvals = self
            .repository
            .list_validation_command_approvals_for_scope(
                request.task_id,
                approval_task_version,
                ValidationExecutionScope::ProjectRoot,
            )
            .map_err(repository_error)?;
        let mut ordered_approvals = Vec::with_capacity(2);
        for required in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
            let Some(approval) = approvals.iter().find(|approval| approval.kind == required) else {
                return Err(category_error(FailureCategory::NotFound));
            };
            ordered_approvals.push(approval.clone());
        }

        let project = self
            .repository
            .get_project(task.project_id())
            .map_err(repository_error)?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        let project_identity = self
            .repository
            .get_project_identity(task.project_id())
            .map_err(repository_error)?
            .filter(|identity| identity.confirmed)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let live_root = self
            .filesystem
            .inspect_supported_directory(Path::new(&project.root_path))
            .map_err(port_error)?;
        if live_root.volume_serial_hex != project_identity.root_volume_serial_hex
            || live_root.file_id_hex != project_identity.root_file_id_hex
            || live_root.canonical_path.to_string_lossy() != project.root_path
            || ordered_approvals.iter().any(|approval| {
                approval.execution_scope != ValidationExecutionScope::ProjectRoot
                    || approval.approved_task_version != approval_task_version
                    || approval.target_project_id != Some(task.project_id())
                    || approval.target_project_identity_revision != Some(project_identity.revision)
                    || approval.target_root_volume_serial_hex.as_deref()
                        != Some(project_identity.root_volume_serial_hex.as_str())
                    || approval.target_root_file_id_hex.as_deref()
                        != Some(project_identity.root_file_id_hex.as_str())
            })
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        Ok(PostMergeValidationInputs {
            task: TaskView::from(&task),
            approval_task_version,
            target: ValidationExecutionTarget::ProjectRoot {
                project_id: task.project_id(),
                project_identity_revision: project_identity.revision,
                directory_identity: live_root,
            },
            approvals: ordered_approvals,
        })
    }
}

fn repository_error(error: chatoms_ports::repository::RepositoryError) -> ApplicationError {
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
