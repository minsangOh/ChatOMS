//! Reads the current manual-resolution candidate for a task's
//! `MergeConflict`: whether the staged index in the original checkout is
//! fully resolved, and if so, the content-free digest that binds a user's
//! confirmation to that exact staged state (see
//! `chatoms_ports::manual_merge_resolution`).
//!
//! Deliberately reuses [`MergeConflictInspectionPort::inspect_merge_conflicts`]
//! (Unit 5e-2a) for identity/topology/residue/unmerged-count classification
//! rather than re-implementing it: a `Ready` candidate must satisfy every
//! precondition `ResolvedPendingConfirmation` already does, plus a few this
//! Unit adds (no `MERGE_AUTOSTASH` residue, a safe repository
//! configuration, and no tracked-unstaged/untracked working-tree change).
//! Path and file content never leave adapter-local memory — only a SHA-256
//! digest and commit identity are returned.

use std::{path::Path, time::Duration};

use chatoms_domain::{ProjectId, TaskId};
use chatoms_ports::{
    manual_merge_resolution::{
        ManualMergeResolutionCandidatePort, ManualMergeResolutionCandidateRequest,
        ManualResolutionCandidate, ManualResolutionCandidateOutcome, ManualResolutionDigest,
    },
    merge_conflict_inspection::{
        MergeConflictInspectionOutcome, MergeConflictInspectionPort, MergeConflictInspectionRequest,
    },
};
use sha2::{Digest, Sha256};

use crate::git::{BoundedCaptureOutcome, GitCliAdapter};

const DOMAIN_TAG: &[u8] = b"chatoms.manual-merge-resolution.v1";

const STATUS_CAPTURE_MAX_BYTES: usize = 256 * 1024;
const STATUS_CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
const INDEX_CAPTURE_MAX_BYTES: usize = 8 * 1024 * 1024;
const INDEX_CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

impl ManualMergeResolutionCandidatePort for GitCliAdapter {
    fn resolution_candidate(
        &mut self,
        request: &ManualMergeResolutionCandidateRequest,
    ) -> ManualResolutionCandidateOutcome {
        match candidate(self, request) {
            Ok(outcome) => outcome,
            Err(()) => ManualResolutionCandidateOutcome::Unavailable,
        }
    }
}

fn candidate(
    git: &mut GitCliAdapter,
    request: &ManualMergeResolutionCandidateRequest,
) -> Result<ManualResolutionCandidateOutcome, ()> {
    let inspection = git.inspect_merge_conflicts(&MergeConflictInspectionRequest {
        original_checkout: request.original_checkout.clone(),
        original_common_dir: request.original_common_dir.clone(),
        task_worktree: request.task_worktree.clone(),
        task_branch: request.task_branch.clone(),
        base_branch: request.base_branch.clone(),
        base_commit: request.base_commit.clone(),
    });
    match inspection.outcome {
        MergeConflictInspectionOutcome::ConfirmedUnresolved => {
            return Ok(ManualResolutionCandidateOutcome::Unresolved);
        }
        MergeConflictInspectionOutcome::Inconsistent => {
            return Ok(ManualResolutionCandidateOutcome::Inconsistent);
        }
        // No merge is in progress and the repository is already fully
        // restored to base -- there is no unresolved index to confirm a
        // digest against, which is exactly what `Inconsistent` already
        // means for this port: no valid resolution candidate exists here.
        MergeConflictInspectionOutcome::RestoredPendingAbortConfirmation => {
            return Ok(ManualResolutionCandidateOutcome::Inconsistent);
        }
        MergeConflictInspectionOutcome::Unavailable => {
            return Ok(ManualResolutionCandidateOutcome::Unavailable);
        }
        MergeConflictInspectionOutcome::ResolvedPendingConfirmation => {}
    }

    if request
        .original_common_dir
        .canonical_path
        .join("MERGE_AUTOSTASH")
        .exists()
    {
        return Ok(ManualResolutionCandidateOutcome::Inconsistent);
    }
    if git
        .validate_write_configuration(
            &request.original_checkout.canonical_path,
            &request.task_worktree.canonical_path,
            &request.base_commit,
        )
        .is_err()
    {
        return Ok(ManualResolutionCandidateOutcome::Inconsistent);
    }
    match status_is_confirmable(git, &request.original_checkout.canonical_path)? {
        true => {}
        false => return Ok(ManualResolutionCandidateOutcome::Inconsistent),
    }

    let Some(task_commit) = read_single(
        git,
        &request.task_worktree.canonical_path,
        &["rev-parse", "--verify", "HEAD"],
    )?
    else {
        return Ok(ManualResolutionCandidateOutcome::Inconsistent);
    };
    let Some(merge_head) = read_single(
        git,
        &request.original_checkout.canonical_path,
        &["rev-parse", "--verify", "MERGE_HEAD"],
    )?
    else {
        return Ok(ManualResolutionCandidateOutcome::Inconsistent);
    };
    if merge_head != task_commit {
        return Ok(ManualResolutionCandidateOutcome::Inconsistent);
    }

    let fields = DigestEnvelopeFields {
        task_id: request.task_id,
        project_id: request.project_id,
        merge_conflict_task_version: request.merge_conflict_task_version,
        source_approval_task_version: request.source_approval_task_version,
        base_branch: &request.base_branch,
        task_branch: &request.task_branch,
        base_commit: &request.base_commit,
    };
    let Some(digest) = recompute_resolution_digest(
        git,
        &request.original_checkout.canonical_path,
        &fields,
        &task_commit,
        &merge_head,
    )?
    else {
        return Ok(ManualResolutionCandidateOutcome::Unavailable);
    };
    Ok(ManualResolutionCandidateOutcome::Ready(
        ManualResolutionCandidate {
            base_commit: request.base_commit.clone(),
            task_commit,
            merge_head_commit: merge_head,
            resolution_digest: digest,
        },
    ))
}

/// The content-free fields [`compute_digest`] binds a digest to, independent
/// of the request shape a caller happens to have on hand — both
/// [`ManualMergeResolutionCandidateRequest`] and
/// `chatoms_ports::merge_continue::MergeContinueRequest` carry the same
/// values under different field names.
pub(crate) struct DigestEnvelopeFields<'a> {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub merge_conflict_task_version: u64,
    pub source_approval_task_version: u64,
    pub base_branch: &'a str,
    pub task_branch: &'a str,
    pub base_commit: &'a str,
}

/// Reads the current stage-0 index at `root` and, if well-formed, folds it
/// into the canonical digest envelope. Returns `Ok(None)` for a malformed or
/// oversized/timed-out read (never a raw error a caller could mistake for a
/// safe-to-retry condition) and `Err(())` only for a genuine transient
/// failure (spawn/IO). Used both by [`candidate`] (mid-merge) and by
/// `crate::merge_continue` (post-commit, to confirm the committed result
/// still matches the confirmed digest).
pub(crate) fn recompute_resolution_digest(
    git: &mut GitCliAdapter,
    root: &Path,
    fields: &DigestEnvelopeFields<'_>,
    task_commit: &str,
    merge_head: &str,
) -> Result<Option<ManualResolutionDigest>, ()> {
    match read_stage_zero_index(git, root)? {
        IndexReadOutcome::Records(records) => Ok(Some(compute_digest(
            fields,
            task_commit,
            merge_head,
            &records,
        ))),
        IndexReadOutcome::Malformed | IndexReadOutcome::Oversized => Ok(None),
    }
}

/// `true` when `git status --porcelain=v1 --untracked-files=all` reports no
/// tracked-unstaged change and no non-ignored untracked file — a staged
/// merge resolution reports only staged (`X ` where `X != ' '`, `Y == ' '`)
/// entries, which this permits.
fn status_is_confirmable(git: &mut GitCliAdapter, root: &Path) -> Result<bool, ()> {
    let bytes = match git
        .capture_read_only(
            root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            STATUS_CAPTURE_MAX_BYTES,
            STATUS_CAPTURE_TIMEOUT,
        )
        .map_err(|_| ())?
    {
        BoundedCaptureOutcome::Success(bytes) => bytes,
        BoundedCaptureOutcome::ExitFailure
        | BoundedCaptureOutcome::TooLarge
        | BoundedCaptureOutcome::TimedOut
        | BoundedCaptureOutcome::Uncertain => return Err(()),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| ())?;
    for line in text.lines() {
        let bytes = line.as_bytes();
        if bytes.len() < 2 {
            continue;
        }
        let (x, y) = (bytes[0], bytes[1]);
        if (x == b'?' && y == b'?') || y != b' ' {
            return Ok(false);
        }
    }
    Ok(true)
}

struct IndexRecord {
    mode: String,
    object_id: String,
    path: Vec<u8>,
}

enum IndexReadOutcome {
    Records(Vec<IndexRecord>),
    Malformed,
    Oversized,
}

fn read_stage_zero_index(git: &mut GitCliAdapter, root: &Path) -> Result<IndexReadOutcome, ()> {
    let bytes = match git
        .capture_read_only(
            root,
            &["ls-files", "--stage", "-z", "--", "."],
            INDEX_CAPTURE_MAX_BYTES,
            INDEX_CAPTURE_TIMEOUT,
        )
        .map_err(|_| ())?
    {
        BoundedCaptureOutcome::Success(bytes) => bytes,
        BoundedCaptureOutcome::ExitFailure => return Ok(IndexReadOutcome::Malformed),
        BoundedCaptureOutcome::TooLarge | BoundedCaptureOutcome::TimedOut => {
            return Ok(IndexReadOutcome::Oversized);
        }
        BoundedCaptureOutcome::Uncertain => return Err(()),
    };
    Ok(match parse_stage_zero_records(&bytes) {
        Some(records) => IndexReadOutcome::Records(records),
        None => IndexReadOutcome::Malformed,
    })
}

fn parse_stage_zero_records(bytes: &[u8]) -> Option<Vec<IndexRecord>> {
    if !bytes.is_empty() && !bytes.ends_with(&[0]) {
        return None;
    }
    let mut records = Vec::new();
    for record in bytes.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        let separator = record.iter().position(|byte| *byte == b'\t')?;
        let (metadata, path_with_separator) = record.split_at(separator);
        let path = &path_with_separator[1..];
        if path.is_empty() {
            return None;
        }
        let fields: Vec<&[u8]> = metadata
            .split(|byte| *byte == b' ')
            .filter(|field| !field.is_empty())
            .collect();
        if fields.len() != 3 {
            return None;
        }
        let mode = std::str::from_utf8(fields[0]).ok()?.to_owned();
        let object_id = std::str::from_utf8(fields[1]).ok()?.to_owned();
        if fields[2] != b"0" || !valid_mode(&mode) || !valid_object_id(&object_id) {
            return None;
        }
        records.push(IndexRecord {
            mode,
            object_id,
            path: path.to_vec(),
        });
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    if records.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return None;
    }
    Some(records)
}

fn valid_mode(value: &str) -> bool {
    matches!(value, "100644" | "100755" | "120000" | "160000")
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

fn compute_digest(
    fields: &DigestEnvelopeFields<'_>,
    task_commit: &str,
    merge_head: &str,
    records: &[IndexRecord],
) -> ManualResolutionDigest {
    let mut buffer = Vec::new();
    push_bytes(&mut buffer, DOMAIN_TAG);
    push_bytes(&mut buffer, fields.task_id.to_string().as_bytes());
    push_bytes(&mut buffer, fields.project_id.to_string().as_bytes());
    push_bytes(
        &mut buffer,
        fields.merge_conflict_task_version.to_string().as_bytes(),
    );
    push_bytes(
        &mut buffer,
        fields.source_approval_task_version.to_string().as_bytes(),
    );
    push_bytes(&mut buffer, fields.base_branch.as_bytes());
    push_bytes(&mut buffer, fields.task_branch.as_bytes());
    push_bytes(&mut buffer, fields.base_commit.as_bytes());
    push_bytes(&mut buffer, task_commit.as_bytes());
    push_bytes(&mut buffer, merge_head.as_bytes());
    push_u64(&mut buffer, records.len() as u64);
    for record in records {
        push_bytes(&mut buffer, record.mode.as_bytes());
        push_bytes(&mut buffer, record.object_id.as_bytes());
        push_bytes(&mut buffer, &record.path);
    }
    let digest = Sha256::digest(&buffer);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    ManualResolutionDigest::from_digest_bytes(bytes)
}

fn push_bytes(buffer: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(buffer, bytes.len() as u64);
    buffer.extend_from_slice(bytes);
}

fn push_u64(buffer: &mut Vec<u8>, value: u64) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn read_single(
    git: &mut GitCliAdapter,
    root: &Path,
    arguments: &[&str],
) -> Result<Option<String>, ()> {
    match git
        .capture_read_only(
            root,
            arguments,
            STATUS_CAPTURE_MAX_BYTES,
            STATUS_CAPTURE_TIMEOUT,
        )
        .map_err(|_| ())?
    {
        BoundedCaptureOutcome::Success(bytes) => String::from_utf8(bytes)
            .map(|text| Some(text.trim().to_owned()))
            .map_err(|_| ()),
        BoundedCaptureOutcome::ExitFailure => Ok(None),
        BoundedCaptureOutcome::TooLarge
        | BoundedCaptureOutcome::TimedOut
        | BoundedCaptureOutcome::Uncertain => Err(()),
    }
}
