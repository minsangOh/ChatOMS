use chatoms_domain::{OperationRiskKind, TargetIdentityDigest, TaskId};

pub trait ProviderImplementationPolicyBinding: Send {
    fn task_id(&self) -> TaskId;

    fn approved_task_version(&self) -> u64;

    fn operation_kind(&self) -> OperationRiskKind;

    fn target_identity_digest(&self) -> TargetIdentityDigest;

    fn matches_worktree_object_identity(&self, volume_serial_hex: &str, file_id_hex: &str) -> bool;
}
