use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    git::GitService,
    merge_conflict_inspection::{
        MergeConflictInspectionOutcome, MergeConflictInspectionPort, MergeConflictInspectionRequest,
    },
};

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("run fixture Git command");
    assert!(output.status.success(), "fixture Git command must succeed");
    String::from_utf8(output.stdout)
        .expect("fixture Git stdout must be UTF-8")
        .trim()
        .to_owned()
}

fn git_with_input(root: &Path, arguments: &[&str], input: &[u8]) {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fixture Git command");
    let mut stdin = child.stdin.take().expect("fixture stdin");
    std::io::Write::write_all(&mut stdin, input).expect("write fixture Git input");
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("wait for fixture Git command");
    assert!(output.status.success(), "fixture Git command must succeed");
}

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary repository");
    git(directory.path(), &["init", "-q", "-b", "main"]);
    git(directory.path(), &["config", "user.name", "ChatOMS Test"]);
    git(
        directory.path(),
        &["config", "user.email", "chatoms@example.invalid"],
    );
    fs::write(directory.path().join("tracked.txt"), "base\n").expect("base file");
    git(directory.path(), &["add", "tracked.txt"]);
    git(directory.path(), &["commit", "-qm", "test: base"]);
    directory
}

fn prepared_conflict() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    GitCliAdapter,
    MergeConflictInspectionRequest,
) {
    let root = repository();
    let worktree_parent = tempfile::tempdir().expect("worktree parent");
    let worktree = worktree_parent.path().join("task-worktree");
    let control = tempfile::tempdir().expect("Git control root");
    let mut adapter = GitCliAdapter::new(control.path().to_path_buf()).expect("Git adapter");
    let base_commit = git(root.path(), &["rev-parse", "HEAD"]);
    let safety = adapter
        .validate_repository_source(root.path(), &base_commit)
        .expect("repository safety evidence");
    let task_branch = "ai-task/merge-conflict".to_owned();
    adapter
        .create_task_worktree(root.path(), &task_branch, &base_commit, &worktree, &safety)
        .expect("create task worktree");
    fs::write(worktree.join("tracked.txt"), "theirs\n").expect("task file");
    git(&worktree, &["add", "tracked.txt"]);
    git(&worktree, &["commit", "-qm", "test: task commit"]);
    let task_commit = git(&worktree, &["rev-parse", "HEAD"]);
    let base_blob = git(root.path(), &["rev-parse", "HEAD:tracked.txt"]);
    fs::write(root.path().join("ours.txt"), "ours\n").expect("ours blob source");
    let ours_blob = git(root.path(), &["hash-object", "-w", "ours.txt"]);
    fs::remove_file(root.path().join("ours.txt")).expect("remove blob source");
    let theirs_blob = git(&worktree, &["rev-parse", "HEAD:tracked.txt"]);
    git_with_input(
        root.path(),
        &["update-index", "--index-info"],
        format!(
            "100644 {base_blob} 1\ttracked.txt\n100644 {ours_blob} 2\ttracked.txt\n100644 {theirs_blob} 3\ttracked.txt\n"
        )
        .as_bytes(),
    );
    fs::write(
        root.path().join(".git").join("MERGE_HEAD"),
        format!("{task_commit}\n"),
    )
    .expect("merge head");
    let request = MergeConflictInspectionRequest {
        original_checkout: supported_directory_identity(root.path()).expect("root identity"),
        original_common_dir: supported_directory_identity(&root.path().join(".git"))
            .expect("common identity"),
        task_worktree: supported_directory_identity(&worktree).expect("worktree identity"),
        task_branch,
        base_branch: "main".to_owned(),
        base_commit,
    };
    (root, worktree_parent, control, adapter, request)
}

#[test]
fn inspection_counts_unmerged_entries_without_exposing_content() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    let result = adapter.inspect_merge_conflicts(&request);
    assert_eq!(
        result.outcome,
        MergeConflictInspectionOutcome::ConfirmedUnresolved
    );
    assert_eq!(result.counts.total, 1);
    assert_eq!(result.counts.both_modified, 1);
    let debug = format!("{result:?}");
    assert!(!debug.contains("tracked.txt"));
    assert!(!debug.contains(&request.base_commit));
    assert!(
        !root
            .path()
            .join(".git")
            .join("MERGE_HEAD")
            .to_string_lossy()
            .is_empty()
    );
}

#[test]
fn inspection_reports_resolved_entries_until_a_future_merge_confirmation() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    fs::write(root.path().join("tracked.txt"), "resolved\n").expect("resolved file");
    let resolved_blob = git(root.path(), &["hash-object", "-w", "tracked.txt"]);
    git(
        root.path(),
        &["update-index", "--force-remove", "--", "tracked.txt"],
    );
    git(
        root.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{resolved_blob},tracked.txt"),
        ],
    );
    let result = adapter.inspect_merge_conflicts(&request);
    assert_eq!(
        result.outcome,
        MergeConflictInspectionOutcome::ResolvedPendingConfirmation
    );
    assert_eq!(result.counts.total, 0);
}

#[test]
fn inspection_rejects_wrong_merge_head_without_writing_repository_state() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_conflict();
    fs::write(
        root.path().join(".git").join("MERGE_HEAD"),
        format!("{}\n", "0".repeat(40)),
    )
    .expect("wrong merge head");
    let result = adapter.inspect_merge_conflicts(&request);
    assert_eq!(result.outcome, MergeConflictInspectionOutcome::Inconsistent);
    request.base_branch = "wrong-base".to_owned();
    let result = adapter.inspect_merge_conflicts(&request);
    assert_eq!(result.outcome, MergeConflictInspectionOutcome::Inconsistent);
}

#[test]
fn inspection_maps_unavailable_live_identity_to_unavailable() {
    let (_root, _worktree_parent, _control, mut adapter, mut request) = prepared_conflict();
    request.task_worktree.canonical_path = request
        .task_worktree
        .canonical_path
        .join("missing-task-worktree");

    let result = adapter.inspect_merge_conflicts(&request);

    assert_eq!(result.outcome, MergeConflictInspectionOutcome::Unavailable);
    assert_eq!(result.counts.total, 0);
}
