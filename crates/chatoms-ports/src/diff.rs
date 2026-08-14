//! Port boundary for reading a task worktree's current Git diff. This
//! exists solely so a future Claude Review adapter can pass the diff to the
//! provider as ephemeral stdin (see `docs/SECURITY_POLICY.md`): the diff
//! text this port returns must never be persisted to SQLite, placed on a
//! DTO/IPC surface, or written to a log. It lives only in bounded,
//! in-process memory for the lifetime of a single read.
//!
//! Implementations are expected to reuse the same trusted Git execution
//! boundary as [`crate::git::GitService`] (same executable trust, same
//! `env_clear`'d environment, no external diff driver/textconv/pager). Kept
//! as its own narrow trait rather than a new [`crate::git::GitService`]
//! method: `GitService` is a general-purpose Git isolation port, and this is
//! a single, Review-specific read.

use std::path::Path;

use crate::error::PortFailure;

/// A worktree's current Git diff text, already confirmed non-empty and
/// within the port's byte bound. `Debug` deliberately reports only a byte
/// count, not the text itself, so a stray `{:?}` in a log statement cannot
/// leak diff content.
pub struct WorktreeDiff {
    text: String,
}

impl WorktreeDiff {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self { text }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Debug for WorktreeDiff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorktreeDiff")
            .field("byte_len", &self.text.len())
            .finish()
    }
}

impl PartialEq for WorktreeDiff {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for WorktreeDiff {}

/// Classification of a single current-worktree-diff read. `TimedOut` and
/// `Uncertain` are kept as outcomes here rather than [`PortFailure`] errors,
/// mirroring [`crate::validation_execution::ValidationExecutionOutcome`]:
/// they are confirmed, safe-to-classify dispositions of a read-only
/// command, not infrastructure-level failures. A genuine spawn failure,
/// non-zero Git exit, or malformed/non-UTF-8 output is returned as
/// `Err(PortFailure)` instead.
#[derive(Debug, Eq, PartialEq)]
pub enum WorktreeDiffOutcome {
    Diff(WorktreeDiff),
    NoChanges,
    DiffTooLarge,
    TimedOut,
    Uncertain,
}

/// Reads the current combined staged+unstaged Git diff of `worktree`
/// against its own `HEAD`, bounded in size and wall-clock time. Never
/// accepts an arbitrary caller-supplied revision or path outside
/// `worktree`. Callers are responsible for revalidating the worktree's
/// identity (e.g. via [`crate::git::GitService::verify_task_worktree`] plus
/// [`crate::filesystem::FilesystemIdentityPort`]) *before* calling this —
/// this port trusts the path it is given and never mutates the repository.
pub trait WorktreeDiffPort {
    fn current_diff(&mut self, worktree: &Path) -> Result<WorktreeDiffOutcome, PortFailure>;
}
