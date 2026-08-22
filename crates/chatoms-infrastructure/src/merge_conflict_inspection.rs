use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    git::GitService,
    merge_conflict_inspection::{
        MergeConflictCounts, MergeConflictInspectionOutcome, MergeConflictInspectionPort,
        MergeConflictInspectionRequest, MergeConflictInspectionResult,
    },
};

use crate::git::{BoundedCaptureOutcome, GitCliAdapter};

#[path = "merge_conflict_parser.rs"]
mod merge_conflict_parser;

const CAPTURE_MAX_BYTES: usize = 256 * 1024;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

impl MergeConflictInspectionPort for GitCliAdapter {
    fn inspect_merge_conflicts(
        &mut self,
        request: &MergeConflictInspectionRequest,
    ) -> MergeConflictInspectionResult {
        match inspect(self, request) {
            Ok(result) => result,
            Err(()) => result(MergeConflictInspectionOutcome::Unavailable),
        }
    }
}

fn inspect(
    git: &mut GitCliAdapter,
    request: &MergeConflictInspectionRequest,
) -> Result<MergeConflictInspectionResult, ()> {
    match identities_match(request)? {
        true => {}
        false => return Ok(result(MergeConflictInspectionOutcome::Inconsistent)),
    }
    if operation_residue_exists(request)? {
        return Ok(result(MergeConflictInspectionOutcome::Inconsistent));
    }
    if !matches_repository(git, request)? {
        return Ok(result(MergeConflictInspectionOutcome::Inconsistent));
    }
    let Some(merge_head) = read_single(
        git,
        &request.original_checkout.canonical_path,
        &["rev-parse", "--verify", "MERGE_HEAD"],
    )?
    else {
        // No merge is currently in progress. `matches_repository` above
        // already confirmed the original checkout is exactly on
        // `base_branch`/`base_commit` and the task worktree is exactly on
        // `task_branch` at a commit whose sole parent is `base_commit`, and
        // the earlier `operation_residue_exists` check already confirmed no
        // foreign rebase/cherry-pick/revert/bisect/sequencer residue -- what
        // remains is confirming no `MERGE_*` residue and a clean original
        // checkout, the exact postcondition a successful `git merge --abort`
        // leaves behind.
        return Ok(if has_merge_residue(request) {
            result(MergeConflictInspectionOutcome::Inconsistent)
        } else {
            match git.repository_status(&request.original_checkout.canonical_path) {
                Ok(status) if status.clean => {
                    result(MergeConflictInspectionOutcome::RestoredPendingAbortConfirmation)
                }
                Ok(_) => result(MergeConflictInspectionOutcome::Inconsistent),
                Err(_) => result(MergeConflictInspectionOutcome::Unavailable),
            }
        });
    };
    let Some(task_commit) = read_single(
        git,
        &request.task_worktree.canonical_path,
        &["rev-parse", "--verify", "HEAD"],
    )?
    else {
        return Ok(result(MergeConflictInspectionOutcome::Inconsistent));
    };
    if merge_head != task_commit || !valid_object_id(&task_commit) {
        return Ok(result(MergeConflictInspectionOutcome::Inconsistent));
    }
    let counts =
        merge_conflict_parser::parse_unmerged(git, &request.original_checkout.canonical_path)?;
    let outcome = if counts.total == 0 {
        MergeConflictInspectionOutcome::ResolvedPendingConfirmation
    } else {
        MergeConflictInspectionOutcome::ConfirmedUnresolved
    };
    Ok(MergeConflictInspectionResult { outcome, counts })
}

fn matches_repository(
    git: &mut GitCliAdapter,
    request: &MergeConflictInspectionRequest,
) -> Result<bool, ()> {
    let Some(root_branch) = read_single(
        git,
        &request.original_checkout.canonical_path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )?
    else {
        return Ok(false);
    };
    let Some(root_head) = read_single(
        git,
        &request.original_checkout.canonical_path,
        &["rev-parse", "--verify", "HEAD"],
    )?
    else {
        return Ok(false);
    };
    if root_branch != request.base_branch || root_head != request.base_commit {
        return Ok(false);
    }
    let Some(task_branch) = read_single(
        git,
        &request.task_worktree.canonical_path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )?
    else {
        return Ok(false);
    };
    let Some(parents) = read_single(
        git,
        &request.task_worktree.canonical_path,
        &["rev-list", "--parents", "-n", "1", "HEAD"],
    )?
    else {
        return Ok(false);
    };
    let parent_parts: Vec<&str> = parents.split_whitespace().collect();
    if task_branch != request.task_branch
        || parent_parts.len() != 2
        || parent_parts[1] != request.base_commit
    {
        return Ok(false);
    }
    let Some(root_path) = read_path(
        git,
        &request.original_checkout.canonical_path,
        &["rev-parse", "--show-toplevel"],
    )?
    else {
        return Ok(false);
    };
    let Some(common_path) = read_path(
        git,
        &request.task_worktree.canonical_path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?
    else {
        return Ok(false);
    };
    let Some(worktree_path) = read_path(
        git,
        &request.task_worktree.canonical_path,
        &["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )?
    else {
        return Ok(false);
    };
    let Some(worktree_git_dir) = read_path(
        git,
        &request.task_worktree.canonical_path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    )?
    else {
        return Ok(false);
    };
    let matches = root_path == request.original_checkout.canonical_path
        && common_path == request.original_common_dir.canonical_path
        && worktree_path == request.task_worktree.canonical_path
        && worktree_git_dir
            .starts_with(request.original_common_dir.canonical_path.join("worktrees"));
    Ok(matches)
}

fn operation_residue_exists(request: &MergeConflictInspectionRequest) -> Result<bool, ()> {
    for name in [
        "REBASE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "BISECT_START",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        match fs::symlink_metadata(request.original_common_dir.canonical_path.join(name)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(()),
        }
    }
    Ok(false)
}

/// Whether any `MERGE_*` file is present in the original checkout's common
/// dir. Distinct from [`operation_residue_exists`], which only checks
/// foreign (non-merge) operations -- a `MERGE_HEAD` whose object is
/// unreadable (so `rev-parse --verify MERGE_HEAD` fails, landing the caller
/// in this "not currently merging" branch) still leaves this `true`, which
/// is exactly the corrupted-but-not-restored case that must stay
/// `Inconsistent` rather than being read as
/// [`MergeConflictInspectionOutcome::RestoredPendingAbortConfirmation`].
fn has_merge_residue(request: &MergeConflictInspectionRequest) -> bool {
    ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE", "MERGE_AUTOSTASH"]
        .iter()
        .any(|name| {
            request
                .original_common_dir
                .canonical_path
                .join(name)
                .exists()
        })
}

fn identities_match(request: &MergeConflictInspectionRequest) -> Result<bool, ()> {
    for expected in [
        &request.original_checkout,
        &request.original_common_dir,
        &request.task_worktree,
    ] {
        let actual = supported_directory_identity(&expected.canonical_path).map_err(|_| ())?;
        if actual.canonical_path != expected.canonical_path || !actual.same_object(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_single(
    git: &mut GitCliAdapter,
    root: &Path,
    arguments: &[&str],
) -> Result<Option<String>, ()> {
    match git
        .capture_read_only(root, arguments, CAPTURE_MAX_BYTES, CAPTURE_TIMEOUT)
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

fn read_path(
    git: &mut GitCliAdapter,
    root: &Path,
    arguments: &[&str],
) -> Result<Option<PathBuf>, ()> {
    let Some(text) = read_single(git, root, arguments)? else {
        return Ok(None);
    };
    supported_directory_identity(Path::new(&text))
        .map(|identity| Some(identity.canonical_path))
        .map_err(|_| ())
}

fn result(outcome: MergeConflictInspectionOutcome) -> MergeConflictInspectionResult {
    MergeConflictInspectionResult {
        outcome,
        counts: MergeConflictCounts::default(),
    }
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
