use serde::Serialize;

/// Content-free response for `confirm_merge_abort_and_start`. Never carries
/// task state, version, approval identity, or any Git/path/content field --
/// the caller relies on the existing task-isolation polling to observe the
/// eventual `MergeConflict -> Cancelled` transition (or the task remaining
/// `MergeConflict` on a pre-write rejection or uncertain outcome).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeAbortStartDto {
    /// `true` if a background abort attempt was started by this call.
    /// `false` if another abort attempt for the same task was already
    /// in flight -- not an error, and no second attempt was started.
    pub started: bool,
}
