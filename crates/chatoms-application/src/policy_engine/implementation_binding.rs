use chatoms_domain::{OperationRiskKind, TargetIdentityDigest, TaskId};
use chatoms_ports::{
    error::FailureCategory, filesystem::FilesystemIdentityPort,
    provider_implementation_policy::ProviderImplementationPolicyBinding,
    repository::FoundationRepository,
};

use super::{
    PolicyDecision, PolicyDenialReason, PolicyEngine, PolicyEvaluationRequest, PolicyOperation,
    category_error,
};
use crate::error::ApplicationError;

/// In-memory capability issued only by [`PolicyEngine`].
///
/// It deliberately has no public constructor or fields and does not implement
/// `Clone`, `Debug`, `Serialize`, or `Deserialize`. No persistence, DTO, or IPC
/// conversion is defined.
///
/// ```compile_fail
/// use chatoms_application::policy_engine::PolicyPermit;
/// fn clone_permit(permit: PolicyPermit) { let _ = permit.clone(); }
/// ```
///
/// ```compile_fail
/// use chatoms_application::policy_engine::PolicyPermit;
/// fn debug_permit(permit: &PolicyPermit) { let _ = format!("{permit:?}"); }
/// ```
///
/// ```compile_fail
/// use chatoms_application::policy_engine::PolicyPermit;
/// fn expose_task_id(permit: PolicyPermit) { let _ = permit.task_id; }
/// ```
pub struct PolicyPermit {
    pub(super) task_id: TaskId,
    pub(super) approved_task_version: u64,
    pub(super) operation_kind: OperationRiskKind,
    pub(super) target_identity_digest: TargetIdentityDigest,
    pub(super) worktree_volume_serial_hex: String,
    pub(super) worktree_file_id_hex: String,
}

impl PolicyPermit {
    pub(super) fn matches_provider_implementation(
        &self,
        task_id: TaskId,
        expected_version: u64,
        target_identity_digest: TargetIdentityDigest,
    ) -> bool {
        self.task_id == task_id
            && self.approved_task_version == expected_version
            && self.operation_kind == OperationRiskKind::ProviderImplementation
            && self.target_identity_digest == target_identity_digest
    }
}

impl ProviderImplementationPolicyBinding for PolicyPermit {
    fn task_id(&self) -> TaskId {
        self.task_id
    }

    fn approved_task_version(&self) -> u64 {
        self.approved_task_version
    }

    fn operation_kind(&self) -> OperationRiskKind {
        self.operation_kind
    }

    fn target_identity_digest(&self) -> TargetIdentityDigest {
        self.target_identity_digest
    }

    fn matches_worktree_object_identity(&self, volume_serial_hex: &str, file_id_hex: &str) -> bool {
        self.worktree_volume_serial_hex == volume_serial_hex
            && self.worktree_file_id_hex == file_id_hex
    }
}

pub(crate) fn require_provider_implementation_permit<R, F>(
    repository: &mut R,
    filesystem: &mut F,
    task_id: TaskId,
    expected_version: u64,
) -> Result<PolicyPermit, ApplicationError>
where
    R: FoundationRepository,
    F: FilesystemIdentityPort,
{
    match PolicyEngine::new(repository, filesystem).evaluate(PolicyEvaluationRequest {
        task_id,
        expected_version,
        operation: PolicyOperation::ProviderImplementation,
    })? {
        PolicyDecision::Authorized(permit) => Ok(permit),
        PolicyDecision::AssessmentRequired => Err(category_error(FailureCategory::InvalidState)),
        PolicyDecision::Denied(reason) => Err(category_error(match reason {
            PolicyDenialReason::UnsupportedOperation => FailureCategory::InvariantViolation,
            PolicyDenialReason::TaskNotFound => FailureCategory::NotFound,
            PolicyDenialReason::VersionMismatch => FailureCategory::VersionConflict,
            PolicyDenialReason::StateMismatch | PolicyDenialReason::ApprovalMissing => {
                FailureCategory::InvalidState
            }
            PolicyDenialReason::LeaseMismatch => FailureCategory::ActiveLeaseConflict,
            PolicyDenialReason::TargetIdentityMismatch => FailureCategory::Conflict,
        })),
    }
}
