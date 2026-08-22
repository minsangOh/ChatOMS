use std::collections::HashMap;

use chatoms_ports::merge_conflict_inspection::{MergeConflictCounts, MergeConflictKind};

use crate::git::{BoundedCaptureOutcome, GitCliAdapter};

use super::{CAPTURE_MAX_BYTES, CAPTURE_TIMEOUT};

pub(super) fn parse_unmerged(
    git: &mut GitCliAdapter,
    root: &std::path::Path,
) -> Result<MergeConflictCounts, ()> {
    let output = match git
        .capture_read_only(
            root,
            &["ls-files", "--unmerged", "-z", "--", "."],
            CAPTURE_MAX_BYTES,
            CAPTURE_TIMEOUT,
        )
        .map_err(|_| ())?
    {
        BoundedCaptureOutcome::Success(bytes) => bytes,
        BoundedCaptureOutcome::ExitFailure
        | BoundedCaptureOutcome::TooLarge
        | BoundedCaptureOutcome::TimedOut
        | BoundedCaptureOutcome::Uncertain => return Err(()),
    };
    if !output.is_empty() && !output.ends_with(&[0]) {
        return Err(());
    }

    let mut stages = HashMap::<Vec<u8>, u8>::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Some(separator) = record.iter().position(|byte| *byte == b'\t') else {
            return Err(());
        };
        let (metadata, path_with_separator) = record.split_at(separator);
        let path = &path_with_separator[1..];
        if path.is_empty() {
            return Err(());
        }
        let fields: Vec<&[u8]> = metadata
            .split(|byte| *byte == b' ')
            .filter(|field| !field.is_empty())
            .collect();
        if fields.len() != 3 || !valid_mode(fields[0]) || !valid_object_id(fields[1]) {
            return Err(());
        }
        let stage = match fields[2] {
            b"1" => 1,
            b"2" => 2,
            b"3" => 3,
            _ => return Err(()),
        };
        let entry = stages.entry(path.to_vec()).or_insert(0);
        let bit = 1_u8 << (stage - 1);
        if *entry & bit != 0 {
            return Err(());
        }
        *entry |= bit;
    }

    let mut counts = MergeConflictCounts::default();
    for stage_set in stages.into_values() {
        let kind = match stage_set {
            0b111 => MergeConflictKind::BothModified,
            0b110 => MergeConflictKind::BothAdded,
            0b001 => MergeConflictKind::BothDeleted,
            0b010 => MergeConflictKind::AddedByUs,
            0b100 => MergeConflictKind::AddedByThem,
            0b011 => MergeConflictKind::DeletedByThem,
            0b101 => MergeConflictKind::DeletedByUs,
            _ => return Err(()),
        };
        if !counts.record(kind) {
            return Err(());
        }
    }
    Ok(counts)
}

fn valid_object_id(value: &[u8]) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn valid_mode(value: &[u8]) -> bool {
    matches!(
        value,
        b"000000" | b"100644" | b"100755" | b"120000" | b"160000"
    )
}
