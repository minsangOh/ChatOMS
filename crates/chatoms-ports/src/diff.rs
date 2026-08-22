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

/// A content-free SHA-256 digest of a diff's exact UTF-8 bytes. Carries only
/// the 32 raw digest bytes — never the diff text itself — so, unlike
/// [`WorktreeDiff`], it is safe to store, log, or return over an IPC/DTO
/// surface. This type deliberately has no dependency on a hashing crate: it
/// only knows how to hold and hex-encode/decode a digest a caller already
/// computed (e.g. via `sha2::Sha256`), so adding it here never requires a
/// new dependency on this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DiffContentHash([u8; 32]);

impl DiffContentHash {
    /// Builds a hash from 32 raw SHA-256 digest bytes a caller already
    /// computed.
    #[must_use]
    pub const fn from_digest_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parses a lowercase, exactly-64-character hex-encoded SHA-256 digest.
    /// Rejects anything else (wrong length, uppercase, non-hex characters)
    /// rather than normalizing it — an approval's hash binding must match
    /// the exact bytes it was computed from, not a case-folded or
    /// best-effort reading of the input.
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 || !hex.bytes().all(is_lowercase_hex_digit) {
            return None;
        }
        let mut bytes = [0u8; 32];
        let hex_bytes = hex.as_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = hex_nibble(hex_bytes[index * 2])?;
            let low = hex_nibble(hex_bytes[index * 2 + 1])?;
            *byte = (high << 4) | low;
        }
        Some(Self(bytes))
    }

    /// Renders the digest as lowercase hex, the only persisted/wire form
    /// this type ever produces.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

fn is_lowercase_hex_digit(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod diff_content_hash_tests {
    use super::DiffContentHash;

    #[test]
    fn digest_bytes_round_trip_through_hex() {
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let hash = DiffContentHash::from_digest_bytes(bytes);
        let hex = hash.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(
            DiffContentHash::from_hex(&hex).expect("valid hex must parse"),
            hash
        );
    }

    #[test]
    fn from_hex_rejects_malformed_input() {
        for malformed in [
            "",
            "not-hex-at-all-not-hex-at-all-not-hex-at-all-not-hex-at-all000",
            &"a".repeat(63),
            &"a".repeat(65),
            &"A".repeat(64),
            &"g".repeat(64),
        ] {
            assert!(
                DiffContentHash::from_hex(malformed).is_none(),
                "must reject malformed input: {malformed:?}"
            );
        }
    }

    #[test]
    fn different_bytes_produce_different_hex_and_are_not_equal() {
        let a = DiffContentHash::from_digest_bytes([1u8; 32]);
        let b = DiffContentHash::from_digest_bytes([2u8; 32]);
        assert_ne!(a, b);
        assert_ne!(a.to_hex(), b.to_hex());
    }
}

/// The complete, read-only candidate set that `git add -A -- .` would stage
/// for a task worktree: the tracked `HEAD` diff plus eligible untracked files.
/// The text is ephemeral and may only reach the dedicated local-user review
/// surface; its digest is the safe approval identity.
pub struct CommitCandidate {
    text: String,
    content_hash: DiffContentHash,
}

impl CommitCandidate {
    #[must_use]
    pub fn new(text: String, content_hash: DiffContentHash) -> Self {
        Self { text, content_hash }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn content_hash(&self) -> DiffContentHash {
        self.content_hash
    }
}

impl std::fmt::Debug for CommitCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommitCandidate")
            .field("byte_len", &self.text.len())
            .field("content_hash", &self.content_hash)
            .finish()
    }
}

impl PartialEq for CommitCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.content_hash == other.content_hash
    }
}

impl Eq for CommitCandidate {}

/// Safe dispositions of a canonical commit-candidate read. Other Git or file
/// failures are represented by [`PortFailure`] and never expose raw content.
#[derive(Debug, Eq, PartialEq)]
pub enum CommitCandidateOutcome {
    Candidate(CommitCandidate),
    NoChanges,
    CandidateTooLarge,
    TimedOut,
    Uncertain,
}

/// Reads the exact approval candidate for one task worktree without writing an
/// index or Git object. This remains separate from [`WorktreeDiffPort`]: the
/// latter is Review-only and intentionally excludes untracked files.
pub trait CommitCandidatePort {
    fn current_commit_candidate(
        &mut self,
        root: &Path,
        base_branch: &str,
        task_branch: &str,
        base_commit: &str,
        worktree: &Path,
    ) -> Result<CommitCandidateOutcome, PortFailure>;
}
