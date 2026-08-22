use chatoms_domain::{ProjectId, TargetIdentityDigest, TaskId};
use chatoms_ports::{filesystem::DirectoryIdentity, repository::ProjectFilesystemIdentityRecord};
use sha2::{Digest, Sha256};

pub(crate) struct ProviderImplementationTargetIdentityFacts<'a> {
    pub(crate) task_id: TaskId,
    pub(crate) project_id: ProjectId,
    pub(crate) approved_task_version: u64,
    pub(crate) project_identity: &'a ProjectFilesystemIdentityRecord,
    pub(crate) worktree_identity: &'a DirectoryIdentity,
}

pub(crate) fn derive_provider_implementation_target_identity_digest(
    facts: &ProviderImplementationTargetIdentityFacts<'_>,
) -> TargetIdentityDigest {
    let mut digest = Sha256::new();
    digest.update(b"chatoms.provider-implementation-risk-target.v1");
    digest.update([0]);
    digest.update(facts.task_id.to_string().as_bytes());
    digest.update([0]);
    digest.update(facts.project_id.to_string().as_bytes());
    digest.update([0]);
    digest.update(facts.approved_task_version.to_be_bytes());
    digest.update(facts.project_identity.revision.to_be_bytes());
    digest.update(facts.project_identity.root_volume_serial_hex.as_bytes());
    digest.update([0]);
    digest.update(facts.project_identity.root_file_id_hex.as_bytes());
    digest.update([0]);
    digest.update(facts.worktree_identity.volume_serial_hex.as_bytes());
    digest.update([0]);
    digest.update(facts.worktree_identity.file_id_hex.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    TargetIdentityDigest::from_digest_bytes(bytes)
}
