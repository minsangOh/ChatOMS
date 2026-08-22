use crate::filesystem::DirectoryIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeConflictKind {
    BothModified,
    BothAdded,
    BothDeleted,
    AddedByUs,
    AddedByThem,
    DeletedByUs,
    DeletedByThem,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MergeConflictCounts {
    pub total: u32,
    pub both_modified: u32,
    pub both_added: u32,
    pub both_deleted: u32,
    pub added_by_us: u32,
    pub added_by_them: u32,
    pub deleted_by_us: u32,
    pub deleted_by_them: u32,
}

impl MergeConflictCounts {
    pub fn record(&mut self, kind: MergeConflictKind) -> bool {
        let count = match kind {
            MergeConflictKind::BothModified => &mut self.both_modified,
            MergeConflictKind::BothAdded => &mut self.both_added,
            MergeConflictKind::BothDeleted => &mut self.both_deleted,
            MergeConflictKind::AddedByUs => &mut self.added_by_us,
            MergeConflictKind::AddedByThem => &mut self.added_by_them,
            MergeConflictKind::DeletedByUs => &mut self.deleted_by_us,
            MergeConflictKind::DeletedByThem => &mut self.deleted_by_them,
        };
        let Some(next) = count.checked_add(1) else {
            return false;
        };
        let Some(total) = self.total.checked_add(1) else {
            return false;
        };
        self.total = total;
        *count = next;
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeConflictInspectionOutcome {
    ConfirmedUnresolved,
    ResolvedPendingConfirmation,
    /// No merge is currently in progress, and the original checkout is
    /// independently confirmed fully restored to `base_branch`/`base_commit`
    /// (clean, no `MERGE_*`/foreign-operation residue) with the task
    /// worktree unchanged on `task_branch` at its task commit and every
    /// identity check intact -- the exact postcondition a prior successful
    /// `git merge --abort` leaves behind, whether or not that abort's own
    /// state commit ever landed. Never a loose substitute for
    /// [`Self::Inconsistent`]: any one of those checks failing keeps the
    /// outcome `Inconsistent` (or `Unavailable` on a Git failure), never
    /// this variant.
    RestoredPendingAbortConfirmation,
    Inconsistent,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MergeConflictInspectionResult {
    pub outcome: MergeConflictInspectionOutcome,
    pub counts: MergeConflictCounts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeConflictInspectionRequest {
    pub original_checkout: DirectoryIdentity,
    pub original_common_dir: DirectoryIdentity,
    pub task_worktree: DirectoryIdentity,
    pub task_branch: String,
    pub base_branch: String,
    pub base_commit: String,
}

pub trait MergeConflictInspectionPort {
    fn inspect_merge_conflicts(
        &mut self,
        request: &MergeConflictInspectionRequest,
    ) -> MergeConflictInspectionResult;
}
