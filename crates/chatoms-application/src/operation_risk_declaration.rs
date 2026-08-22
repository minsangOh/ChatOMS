use std::{collections::HashSet, path::Path};

use chatoms_domain::{HighRiskCategory, OperationRiskKind, TaskId, TaskState};
use chatoms_ports::{
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    repository::{
        FoundationRepository, GitIsolationStatus, OperationRiskDeclaration,
        OperationRiskDeclarationRecord,
    },
};

use crate::{
    error::ApplicationError,
    operation_target_identity::{
        ProviderImplementationTargetIdentityFacts,
        derive_provider_implementation_target_identity_digest,
    },
};

pub struct DeclareProviderImplementationRiskRequest {
    pub task_id: TaskId,
    pub expected_version: u64,
    pub risk_categories: Vec<HighRiskCategory>,
    pub declared_at_ms: i64,
}

pub struct OperationRiskDeclarationService<'a, R, F> {
    repository: &'a mut R,
    filesystem: &'a mut F,
}

impl<'a, R, F> OperationRiskDeclarationService<'a, R, F>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
{
    pub fn new(repository: &'a mut R, filesystem: &'a mut F) -> Self {
        Self {
            repository,
            filesystem,
        }
    }

    pub fn declare_provider_implementation_risk(
        &mut self,
        request: DeclareProviderImplementationRiskRequest,
    ) -> Result<OperationRiskDeclaration, ApplicationError> {
        if request.declared_at_ms < 0
            || request
                .risk_categories
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len()
                != request.risk_categories.len()
        {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::AwaitingDesignApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        if self
            .repository
            .active_lease()
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .map(|lease| lease.task_id)
            != Some(task.id())
        {
            return Err(category_error(FailureCategory::ActiveLeaseConflict));
        }
        let project = self
            .repository
            .get_project(task.project_id())
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let project_identity = self
            .repository
            .get_project_identity(task.project_id())
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        if project.id != task.project_id() || !project_identity.confirmed {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let isolation = self
            .repository
            .get_task_isolation(task.id())
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        if isolation.task_id != task.id()
            || isolation.project_id != task.project_id()
            || isolation.status != GitIsolationStatus::WorktreeReady
            || !isolation.branch_created_by_app
            || !isolation.worktree_created_by_app
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let worktree_path = isolation
            .worktree_path
            .as_deref()
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let live_project_root = self
            .filesystem
            .inspect_supported_directory(Path::new(&project.root_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if live_project_root.volume_serial_hex != project_identity.root_volume_serial_hex
            || live_project_root.file_id_hex != project_identity.root_file_id_hex
        {
            return Err(category_error(FailureCategory::Conflict));
        }
        let live_worktree = self
            .filesystem
            .inspect_supported_directory(Path::new(worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        for category in &request.risk_categories {
            if self
                .repository
                .get_high_risk_approval(task.id(), request.expected_version, *category)
                .map_err(|error| ApplicationError::from_categorized(&error))?
                .is_none()
            {
                return Err(category_error(FailureCategory::InvalidState));
            }
        }
        let declaration = OperationRiskDeclarationRecord {
            task_id: task.id(),
            approved_task_version: request.expected_version,
            operation_kind: OperationRiskKind::ProviderImplementation,
            target_identity_digest: derive_provider_implementation_target_identity_digest(
                &ProviderImplementationTargetIdentityFacts {
                    task_id: task.id(),
                    project_id: task.project_id(),
                    approved_task_version: request.expected_version,
                    project_identity: &project_identity,
                    worktree_identity: &live_worktree,
                },
            ),
            declared_at_ms: request.declared_at_ms,
        };
        self.repository
            .declare_operation_risk(&declaration, &request.risk_categories)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(OperationRiskDeclaration {
            record: declaration,
            risk_categories: request.risk_categories,
        })
    }
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
