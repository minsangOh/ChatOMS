use crate::{diff::DiffContentHash, filesystem::DirectoryIdentity};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeExecutionRequest {
    pub original_checkout: DirectoryIdentity,
    pub original_common_dir: DirectoryIdentity,
    pub task_worktree: DirectoryIdentity,
    pub task_branch: String,
    pub base_branch: String,
    pub base_commit: String,
    pub approved_diff_content_hash: DiffContentHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreWriteRejection {
    IdentityOrTopology,
    OriginalCheckoutNotReady,
    ExistingMergeResidue,
    UnsafeRepositoryConfiguration,
    ApprovedCandidateMismatch,
    AuthorUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeExecutionOutcome {
    Merged,
    PreWriteRejected(PreWriteRejection),
    StageWriteUncertain,
    CommitNotCreated,
    CommitSucceededMergeFailed,
    ConfirmedMergeConflict,
    MergeConflictResidue,
    PostWriteUncertain,
}

pub trait MergeExecutionPort {
    fn commit_and_merge(&mut self, request: &MergeExecutionRequest) -> MergeExecutionOutcome;
}
