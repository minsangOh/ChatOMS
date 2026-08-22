//! Content-free identity for a user's confirmation of a manually-resolved
//! `git merge` conflict. This is a distinct approval axis from
//! [`crate::diff::DiffContentHash`] (which binds to the reviewed task diff
//! *before* `Merging` ever starts): a manual resolution can only be
//! confirmed once `MergeConflict` already exists, and it binds to the
//! staged index of the *original checkout* mid-merge, not the task
//! worktree's diff.
//!
//! [`ManualResolutionDigest`] and everything derived from it never carries
//! raw file paths or content — only a SHA-256 digest over a canonical,
//! length-prefixed encoding of task/version/branch/commit identity plus the
//! staged index's `(mode, object id, path)` triples. See
//! `chatoms_infrastructure::manual_merge_resolution` for the adapter that
//! builds this digest from a live repository, and `docs/DECISIONS.md` for
//! the full canonical-envelope contract (`chatoms.manual-merge-resolution.v1`).

use crate::filesystem::DirectoryIdentity;
use chatoms_domain::{ProjectId, TaskId};

/// A content-free SHA-256 digest binding one manual conflict resolution to
/// the exact task/version/branch/commit identity and staged index it was
/// computed from. Deliberately a distinct type from
/// [`crate::diff::DiffContentHash`] — the two approvals answer different
/// questions and must never be compared or substituted for one another.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ManualResolutionDigest([u8; 32]);

impl ManualResolutionDigest {
    #[must_use]
    pub const fn from_digest_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parses a lowercase, exactly-64-character hex-encoded SHA-256 digest.
    /// Rejects anything else rather than normalizing it.
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

/// Read-only request to classify the current manual-resolution state of a
/// task's `MergeConflict`. Carries the same original-checkout/common-dir/
/// task-worktree identity and branch/commit fields as
/// [`crate::merge_conflict_inspection::MergeConflictInspectionRequest`],
/// plus the task/project identity and both task-version components the
/// digest envelope binds to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualMergeResolutionCandidateRequest {
    pub original_checkout: DirectoryIdentity,
    pub original_common_dir: DirectoryIdentity,
    pub task_worktree: DirectoryIdentity,
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub merge_conflict_task_version: u64,
    pub source_approval_task_version: u64,
    pub task_branch: String,
    pub base_branch: String,
    pub base_commit: String,
}

/// The exact content-free identity a `Ready` candidate exposes: the commits
/// the digest is bound to and the digest itself. Never carries a path, file
/// content, or raw Git output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualResolutionCandidate {
    pub base_commit: String,
    pub task_commit: String,
    pub merge_head_commit: String,
    pub resolution_digest: ManualResolutionDigest,
}

/// Closed disposition of a manual-resolution candidate read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManualResolutionCandidateOutcome {
    /// Every precondition holds: identity/topology match, no unresolved
    /// index entries, no rebase/cherry-pick/revert/bisect/sequencer or
    /// `MERGE_AUTOSTASH` residue, no tracked unstaged change, no non-ignored
    /// untracked file, and a safe repository configuration.
    Ready(ManualResolutionCandidate),
    /// Unmerged index entries remain — the conflict has not been resolved.
    Unresolved,
    /// Identity, topology, residue, configuration, or working-tree status
    /// could not be confirmed safe.
    Inconsistent,
    /// A transient read failure (spawn, timeout, oversized capture, or
    /// malformed output) prevented classification.
    Unavailable,
}

/// Reads the current manual-resolution candidate for one task's
/// `MergeConflict`, without writing anything. Implementations must
/// re-verify filesystem/Git identity fresh on every call — never trust a
/// cached or previously-read value.
pub trait ManualMergeResolutionCandidatePort {
    fn resolution_candidate(
        &mut self,
        request: &ManualMergeResolutionCandidateRequest,
    ) -> ManualResolutionCandidateOutcome;
}

#[cfg(test)]
mod manual_resolution_digest_tests {
    use super::ManualResolutionDigest;

    #[test]
    fn digest_bytes_round_trip_through_hex() {
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let digest = ManualResolutionDigest::from_digest_bytes(bytes);
        let hex = digest.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(
            ManualResolutionDigest::from_hex(&hex).expect("valid hex must parse"),
            digest
        );
    }

    #[test]
    fn from_hex_rejects_malformed_input() {
        for malformed in [
            "",
            &"a".repeat(63),
            &"a".repeat(65),
            &"A".repeat(64),
            &"g".repeat(64),
        ] {
            assert!(
                ManualResolutionDigest::from_hex(malformed).is_none(),
                "must reject malformed input: {malformed:?}"
            );
        }
    }

    #[test]
    fn different_bytes_produce_different_hex_and_are_not_equal() {
        let a = ManualResolutionDigest::from_digest_bytes([1u8; 32]);
        let b = ManualResolutionDigest::from_digest_bytes([2u8; 32]);
        assert_ne!(a, b);
        assert_ne!(a.to_hex(), b.to_hex());
    }
}
