use std::{collections::HashSet, path::Path};

use chatoms_domain::{OperationRiskKind, TaskId, TaskState};
use chatoms_ports::{
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    repository::{FoundationRepository, GitIsolationStatus},
};

use crate::{
    error::ApplicationError,
    operation_target_identity::{
        ProviderImplementationTargetIdentityFacts,
        derive_provider_implementation_target_identity_digest,
    },
};

mod implementation_binding;

pub use implementation_binding::PolicyPermit;
pub(crate) use implementation_binding::require_provider_implementation_permit;

/// Closed Policy Engine operation vocabulary for Phase 5g-2c.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyOperation {
    ProviderImplementation,
    /// A closed sentinel for callers that cannot map an operation to the
    /// currently supported vocabulary. It is never an executable operation.
    Unsupported,
}

pub struct PolicyEvaluationRequest {
    pub task_id: TaskId,
    pub expected_version: u64,
    pub operation: PolicyOperation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyDenialReason {
    UnsupportedOperation,
    TaskNotFound,
    VersionMismatch,
    StateMismatch,
    LeaseMismatch,
    ApprovalMissing,
    TargetIdentityMismatch,
}

pub enum PolicyDecision {
    Authorized(PolicyPermit),
    AssessmentRequired,
    Denied(PolicyDenialReason),
}

pub struct PolicyEngine<'a, R, F> {
    repository: &'a mut R,
    filesystem: &'a mut F,
}

impl<'a, R, F> PolicyEngine<'a, R, F>
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

    pub fn evaluate(
        &mut self,
        request: PolicyEvaluationRequest,
    ) -> Result<PolicyDecision, ApplicationError> {
        if request.operation != PolicyOperation::ProviderImplementation {
            return Ok(PolicyDecision::Denied(
                PolicyDenialReason::UnsupportedOperation,
            ));
        }
        let Some(task) = self
            .repository
            .get_task(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
        else {
            return Ok(PolicyDecision::Denied(PolicyDenialReason::TaskNotFound));
        };
        if task.version() != request.expected_version {
            return Ok(PolicyDecision::Denied(PolicyDenialReason::VersionMismatch));
        }
        if task.state() != TaskState::AwaitingDesignApproval {
            return Ok(PolicyDecision::Denied(PolicyDenialReason::StateMismatch));
        }
        if self
            .repository
            .active_lease()
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .map(|lease| lease.task_id)
            != Some(task.id())
        {
            return Ok(PolicyDecision::Denied(PolicyDenialReason::LeaseMismatch));
        }

        let Some(declaration) = self
            .repository
            .get_operation_risk_declaration(
                task.id(),
                request.expected_version,
                OperationRiskKind::ProviderImplementation,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?
        else {
            return Ok(PolicyDecision::AssessmentRequired);
        };
        if declaration.record.task_id != task.id()
            || declaration.record.approved_task_version != request.expected_version
            || declaration.record.operation_kind != OperationRiskKind::ProviderImplementation
            || declaration
                .risk_categories
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len()
                != declaration.risk_categories.len()
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        for category in &declaration.risk_categories {
            let Some(approval) = self
                .repository
                .get_high_risk_approval(task.id(), request.expected_version, *category)
                .map_err(|error| ApplicationError::from_categorized(&error))?
            else {
                return Ok(PolicyDecision::Denied(PolicyDenialReason::ApprovalMissing));
            };
            if approval.task_id != task.id()
                || approval.approved_task_version != request.expected_version
                || approval.risk_category != *category
            {
                return Err(category_error(FailureCategory::InvariantViolation));
            }
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
        if project.id != task.project_id() || project_identity.project_id != task.project_id() {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        if !project_identity.confirmed {
            return Ok(PolicyDecision::Denied(
                PolicyDenialReason::TargetIdentityMismatch,
            ));
        }
        let isolation = self
            .repository
            .get_task_isolation(task.id())
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        if isolation.task_id != task.id() || isolation.project_id != task.project_id() {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        if isolation.status != GitIsolationStatus::WorktreeReady
            || !isolation.branch_created_by_app
            || !isolation.worktree_created_by_app
        {
            return Ok(PolicyDecision::Denied(
                PolicyDenialReason::TargetIdentityMismatch,
            ));
        }
        let worktree_path = isolation
            .worktree_path
            .as_deref()
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let live_project = self
            .filesystem
            .inspect_supported_directory(Path::new(&project.root_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if live_project.volume_serial_hex != project_identity.root_volume_serial_hex
            || live_project.file_id_hex != project_identity.root_file_id_hex
        {
            return Ok(PolicyDecision::Denied(
                PolicyDenialReason::TargetIdentityMismatch,
            ));
        }
        let live_worktree = self
            .filesystem
            .inspect_supported_directory(Path::new(worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let target_identity_digest = derive_provider_implementation_target_identity_digest(
            &ProviderImplementationTargetIdentityFacts {
                task_id: task.id(),
                project_id: task.project_id(),
                approved_task_version: request.expected_version,
                project_identity: &project_identity,
                worktree_identity: &live_worktree,
            },
        );
        if target_identity_digest != declaration.record.target_identity_digest {
            return Ok(PolicyDecision::Denied(
                PolicyDenialReason::TargetIdentityMismatch,
            ));
        }
        let permit = PolicyPermit {
            task_id: task.id(),
            approved_task_version: request.expected_version,
            operation_kind: OperationRiskKind::ProviderImplementation,
            target_identity_digest,
            worktree_volume_serial_hex: live_worktree.volume_serial_hex,
            worktree_file_id_hex: live_worktree.file_id_hex,
        };
        debug_assert!(permit.matches_provider_implementation(
            task.id(),
            request.expected_version,
            target_identity_digest
        ));
        Ok(PolicyDecision::Authorized(permit))
    }
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
