use serde::Serialize;

/// Content-free response for `get_merge_conflict_write_status`.
///
/// Deliberately a single boolean. It never reveals *which* merge-conflict
/// write is running (continue or abort), and carries no path, branch,
/// commit, hash, approval, digest, Git output, error, environment, or
/// auth/session field -- the UI only needs to know whether it must keep its
/// merge-conflict actions withheld.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflictWriteStatusDto {
    /// `true` while a merge-conflict Git write for this task holds the
    /// process-local `MergeConflictWriteLock`.
    pub running: bool,
}
