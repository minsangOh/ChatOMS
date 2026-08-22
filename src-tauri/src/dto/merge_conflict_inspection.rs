use chatoms_ports::merge_conflict_inspection::{
    MergeConflictCounts, MergeConflictInspectionOutcome, MergeConflictInspectionResult,
};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeConflictInspectionOutcomeDto {
    ConfirmedUnresolved,
    ResolvedPendingConfirmation,
    RestoredPendingAbortConfirmation,
    Inconsistent,
    Unavailable,
}

impl From<MergeConflictInspectionOutcome> for MergeConflictInspectionOutcomeDto {
    fn from(value: MergeConflictInspectionOutcome) -> Self {
        match value {
            MergeConflictInspectionOutcome::ConfirmedUnresolved => Self::ConfirmedUnresolved,
            MergeConflictInspectionOutcome::ResolvedPendingConfirmation => {
                Self::ResolvedPendingConfirmation
            }
            MergeConflictInspectionOutcome::RestoredPendingAbortConfirmation => {
                Self::RestoredPendingAbortConfirmation
            }
            MergeConflictInspectionOutcome::Inconsistent => Self::Inconsistent,
            MergeConflictInspectionOutcome::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflictCountsDto {
    pub total: u32,
    pub both_modified: u32,
    pub both_added: u32,
    pub both_deleted: u32,
    pub added_by_us: u32,
    pub added_by_them: u32,
    pub deleted_by_us: u32,
    pub deleted_by_them: u32,
}

impl From<MergeConflictCounts> for MergeConflictCountsDto {
    fn from(value: MergeConflictCounts) -> Self {
        Self {
            total: value.total,
            both_modified: value.both_modified,
            both_added: value.both_added,
            both_deleted: value.both_deleted,
            added_by_us: value.added_by_us,
            added_by_them: value.added_by_them,
            deleted_by_us: value.deleted_by_us,
            deleted_by_them: value.deleted_by_them,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflictInspectionDto {
    pub outcome: MergeConflictInspectionOutcomeDto,
    pub counts: MergeConflictCountsDto,
}

impl From<MergeConflictInspectionResult> for MergeConflictInspectionDto {
    fn from(value: MergeConflictInspectionResult) -> Self {
        Self {
            outcome: value.outcome.into(),
            counts: value.counts.into(),
        }
    }
}

impl MergeConflictInspectionDto {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            outcome: MergeConflictInspectionOutcomeDto::Unavailable,
            counts: MergeConflictCountsDto {
                total: 0,
                both_modified: 0,
                both_added: 0,
                both_deleted: 0,
                added_by_us: 0,
                added_by_them: 0,
                deleted_by_us: 0,
                deleted_by_them: 0,
            },
        }
    }
}
