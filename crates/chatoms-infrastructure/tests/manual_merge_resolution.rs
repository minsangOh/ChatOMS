use std::{fs, path::Path, process::Command};

use chatoms_domain::{ProjectId, TaskId};
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    git::GitService,
    manual_merge_resolution::{
        ManualMergeResolutionCandidatePort, ManualMergeResolutionCandidateRequest,
        ManualResolutionCandidateOutcome,
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
    ManualMergeResolutionCandidateRequest,
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
    let task_branch = "ai-task/manual-resolution".to_owned();
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
    git_update_index_info(
        root.path(),
        &format!(
            "100644 {base_blob} 1\ttracked.txt\n100644 {ours_blob} 2\ttracked.txt\n100644 {theirs_blob} 3\ttracked.txt\n"
        ),
    );
    fs::write(
        root.path().join(".git").join("MERGE_HEAD"),
        format!("{task_commit}\n"),
    )
    .expect("merge head");
    // Matches what a real `--no-ff` conflicted merge always leaves behind
    // (see `merge_execution.rs`'s adapter), for fixture realism.
    fs::write(root.path().join(".git").join("MERGE_MODE"), "no-ff").expect("merge mode");
    let request = ManualMergeResolutionCandidateRequest {
        original_checkout: supported_directory_identity(root.path()).expect("root identity"),
        original_common_dir: supported_directory_identity(&root.path().join(".git"))
            .expect("common identity"),
        task_worktree: supported_directory_identity(&worktree).expect("worktree identity"),
        task_id: TaskId::new(),
        project_id: ProjectId::new(),
        merge_conflict_task_version: 3,
        source_approval_task_version: 1,
        task_branch,
        base_branch: "main".to_owned(),
        base_commit,
    };
    (root, worktree_parent, control, adapter, request)
}

fn git_update_index_info(root: &Path, entries: &str) {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fixture update-index");
    child
        .stdin
        .take()
        .expect("fixture stdin")
        .write_all(entries.as_bytes())
        .expect("write fixture index entries");
    let output = child.wait_with_output().expect("wait for update-index");
    assert!(output.status.success(), "fixture update-index must succeed");
}

fn resolve_with_content(root: &Path, content: &str) {
    fs::write(root.join("tracked.txt"), content).expect("resolved file");
    let blob = git(root, &["hash-object", "-w", "tracked.txt"]);
    git(
        root,
        &["update-index", "--force-remove", "--", "tracked.txt"],
    );
    git(
        root,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},tracked.txt"),
        ],
    );
}

#[test]
fn ready_candidate_has_a_deterministic_content_free_digest() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved\n");

    let first = adapter.resolution_candidate(&request);
    let second = adapter.resolution_candidate(&request);

    let (
        ManualResolutionCandidateOutcome::Ready(first),
        ManualResolutionCandidateOutcome::Ready(second),
    ) = (first, second)
    else {
        panic!("expected a Ready candidate on both reads");
    };
    assert_eq!(
        first, second,
        "the same staged state must digest identically"
    );
    let debug = format!("{first:?}");
    assert!(!debug.contains("tracked.txt"));
    assert!(!debug.contains("resolved"));
}

#[test]
fn different_resolved_content_changes_the_digest() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved-one\n");
    let ManualResolutionCandidateOutcome::Ready(one) = adapter.resolution_candidate(&request)
    else {
        panic!("expected Ready");
    };

    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved-two\n");
    let ManualResolutionCandidateOutcome::Ready(two) = adapter.resolution_candidate(&request)
    else {
        panic!("expected Ready");
    };

    assert_ne!(one.resolution_digest, two.resolution_digest);
}

#[test]
fn different_task_version_changes_the_digest_for_identical_content() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved\n");
    let ManualResolutionCandidateOutcome::Ready(before) = adapter.resolution_candidate(&request)
    else {
        panic!("expected Ready");
    };
    request.merge_conflict_task_version += 1;
    let ManualResolutionCandidateOutcome::Ready(after) = adapter.resolution_candidate(&request)
    else {
        panic!("expected Ready");
    };
    assert_ne!(before.resolution_digest, after.resolution_digest);
}

#[test]
fn unresolved_conflict_markers_report_unresolved_not_ready() {
    let (_root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    let outcome = adapter.resolution_candidate(&request);
    assert_eq!(outcome, ManualResolutionCandidateOutcome::Unresolved);
}

#[test]
fn merge_autostash_residue_is_rejected_even_when_otherwise_resolved() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved\n");
    fs::write(root.path().join(".git").join("MERGE_AUTOSTASH"), "x\n").expect("autostash residue");

    let outcome = adapter.resolution_candidate(&request);

    assert_eq!(outcome, ManualResolutionCandidateOutcome::Inconsistent);
}

#[test]
fn a_tracked_unstaged_change_outside_the_resolution_is_rejected() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved\n");
    fs::write(root.path().join("tracked.txt"), "resolved\nplus extra\n").expect("dirty edit");

    let outcome = adapter.resolution_candidate(&request);

    assert_eq!(outcome, ManualResolutionCandidateOutcome::Inconsistent);
}

#[test]
fn a_non_ignored_untracked_file_is_rejected() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved\n");
    fs::write(root.path().join("untracked.txt"), "stray\n").expect("untracked file");

    let outcome = adapter.resolution_candidate(&request);

    assert_eq!(outcome, ManualResolutionCandidateOutcome::Inconsistent);
}

#[test]
fn wrong_merge_head_or_missing_identity_maps_to_a_safe_outcome() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_conflict();
    resolve_with_content(root.path(), "resolved\n");
    fs::write(
        root.path().join(".git").join("MERGE_HEAD"),
        format!("{}\n", "0".repeat(40)),
    )
    .expect("wrong merge head");
    assert_eq!(
        adapter.resolution_candidate(&request),
        ManualResolutionCandidateOutcome::Inconsistent
    );

    request.task_worktree.canonical_path = request
        .task_worktree
        .canonical_path
        .join("missing-task-worktree");
    assert_eq!(
        adapter.resolution_candidate(&request),
        ManualResolutionCandidateOutcome::Unavailable
    );
}
