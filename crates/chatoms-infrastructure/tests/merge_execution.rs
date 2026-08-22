use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use chatoms_infrastructure::git::{GitCliAdapter, GitWriteCommand, GitWriteCommandObserver};
use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    diff::{CommitCandidateOutcome, CommitCandidatePort},
    git::GitService,
    merge_execution::{
        MergeExecutionOutcome, MergeExecutionPort, MergeExecutionRequest, PreWriteRejection,
    },
};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run fixture git");
    assert!(output.status.success(), "fixture Git command must succeed");
    String::from_utf8(output.stdout)
        .expect("fixture Git stdout must be UTF-8")
        .trim()
        .to_owned()
}

fn adapter() -> (tempfile::TempDir, GitCliAdapter) {
    let control = tempfile::tempdir().expect("Git control root");
    let adapter = GitCliAdapter::new(control.path().to_path_buf()).expect("controlled Git adapter");
    (control, adapter)
}

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary repository");
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "user.name", "ChatOMS Test"]);
    git(
        directory.path(),
        &["config", "user.email", "chatoms@example.invalid"],
    );
    fs::write(directory.path().join("tracked.txt"), "baseline\n").expect("baseline fixture");
    git(directory.path(), &["add", "tracked.txt"]);
    git(directory.path(), &["commit", "-m", "test: baseline"]);
    directory
}

fn prepared_task() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    GitCliAdapter,
    MergeExecutionRequest,
) {
    let root = repository();
    let worktree_parent = tempfile::tempdir().expect("worktree parent");
    let worktree = worktree_parent.path().join("task-worktree");
    let (control, mut adapter) = adapter();
    let base_commit = git(root.path(), &["rev-parse", "HEAD"]);
    let safety = adapter
        .validate_repository_source(root.path(), &base_commit)
        .expect("repository safety evidence");
    let task_branch = "ai-task/merge-test".to_owned();
    adapter
        .create_task_worktree(root.path(), &task_branch, &base_commit, &worktree, &safety)
        .expect("create task worktree");
    let request = MergeExecutionRequest {
        original_checkout: supported_directory_identity(root.path()).expect("root identity"),
        original_common_dir: supported_directory_identity(&root.path().join(".git"))
            .expect("common directory identity"),
        task_worktree: supported_directory_identity(&worktree).expect("worktree identity"),
        task_branch,
        base_branch: "main".to_owned(),
        base_commit,
        approved_diff_content_hash: chatoms_ports::diff::DiffContentHash::from_digest_bytes(
            [0; 32],
        ),
    };
    (root, worktree_parent, control, adapter, request)
}

fn approve_current_candidate(adapter: &mut GitCliAdapter, request: &mut MergeExecutionRequest) {
    let outcome = adapter
        .current_commit_candidate(
            &request.original_checkout.canonical_path,
            &request.base_branch,
            &request.task_branch,
            &request.base_commit,
            &request.task_worktree.canonical_path,
        )
        .expect("candidate calculation");
    let CommitCandidateOutcome::Candidate(candidate) = outcome else {
        panic!("fixture must have a canonical candidate");
    };
    request.approved_diff_content_hash = candidate.content_hash();
}

struct ConflictBeforeMerge {
    root: PathBuf,
    changed: AtomicBool,
}

impl GitWriteCommandObserver for ConflictBeforeMerge {
    fn before_command(&self, command: GitWriteCommand) {
        if command != GitWriteCommand::Merge || self.changed.swap(true, Ordering::SeqCst) {
            return;
        }
        fs::write(self.root.join("tracked.txt"), "competing\n").expect("competing root change");
        git(&self.root, &["add", "tracked.txt"]);
        git(&self.root, &["commit", "-m", "test: competing change"]);
    }
}

/// Detaches the original checkout's `HEAD` immediately before the merge
/// write. Everything the prewrite gate checked (clean checkout, on the base
/// branch, at the base commit) was true when it looked, and the merge itself
/// still succeeds and still produces a correct two-parent commit — only the
/// postcondition can catch that the merge landed on a detached `HEAD`
/// instead of on the expected base branch.
struct DetachHeadBeforeMerge {
    root: PathBuf,
    detached: AtomicBool,
}

impl GitWriteCommandObserver for DetachHeadBeforeMerge {
    fn before_command(&self, command: GitWriteCommand) {
        if command != GitWriteCommand::Merge || self.detached.swap(true, Ordering::SeqCst) {
            return;
        }
        git(&self.root, &["checkout", "--detach"]);
    }
}

struct TimeoutBeforeStage;

impl GitWriteCommandObserver for TimeoutBeforeStage {
    fn before_command(&self, command: GitWriteCommand) {
        if command == GitWriteCommand::Stage {
            thread::sleep(Duration::from_secs(21));
        }
    }
}

#[test]
fn commit_and_merge_merges_only_the_approved_candidate_with_a_no_ff_merge_commit() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved\n",
    )
    .expect("tracked change");
    fs::write(
        request.task_worktree.canonical_path.join("new.txt"),
        "approved new file\n",
    )
    .expect("untracked change");
    approve_current_candidate(&mut adapter, &mut request);

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(outcome, MergeExecutionOutcome::Merged);
    let task_commit = git(
        &request.task_worktree.canonical_path,
        &["rev-parse", "HEAD"],
    );
    let parents = git(root.path(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
    let parent_parts: Vec<_> = parents.split_whitespace().collect();
    assert_eq!(parent_parts.len(), 3);
    assert_eq!(parent_parts[1], request.base_commit);
    assert_eq!(parent_parts[2], task_commit);
    assert_eq!(
        fs::read_to_string(root.path().join("tracked.txt")).expect("merged tracked file"),
        "approved\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("new.txt")).expect("merged new file"),
        "approved new file\n"
    );
}

#[test]
fn commit_and_merge_rejects_a_hash_mismatch_before_any_git_write() {
    let (root, _worktree_parent, _control, mut adapter, request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "unapproved\n",
    )
    .expect("tracked change");

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(
        outcome,
        MergeExecutionOutcome::PreWriteRejected(PreWriteRejection::ApprovedCandidateMismatch)
    );
    assert_eq!(
        git(root.path(), &["rev-parse", "HEAD"]),
        request.base_commit
    );
    assert_eq!(
        git(
            &request.task_worktree.canonical_path,
            &["rev-parse", "HEAD"]
        ),
        request.base_commit
    );
}

#[test]
fn commit_and_merge_rejects_identity_mismatch_before_any_git_write() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved\n",
    )
    .expect("tracked change");
    approve_current_candidate(&mut adapter, &mut request);
    request.task_worktree.file_id_hex = "0000000000000000".to_owned();

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(
        outcome,
        MergeExecutionOutcome::PreWriteRejected(PreWriteRejection::IdentityOrTopology)
    );
    assert_eq!(
        git(root.path(), &["rev-parse", "HEAD"]),
        request.base_commit
    );
}

#[test]
fn commit_and_merge_rejects_a_dirty_original_checkout_before_any_git_write() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved\n",
    )
    .expect("task change");
    approve_current_candidate(&mut adapter, &mut request);
    fs::write(root.path().join("unexpected.txt"), "dirty\n").expect("original checkout dirt");

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(
        outcome,
        MergeExecutionOutcome::PreWriteRejected(PreWriteRejection::OriginalCheckoutNotReady)
    );
    assert_eq!(
        git(
            &request.task_worktree.canonical_path,
            &["rev-parse", "HEAD"]
        ),
        request.base_commit
    );
}

#[test]
fn commit_and_merge_rejects_existing_merge_residue_before_any_git_write() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved\n",
    )
    .expect("task change");
    approve_current_candidate(&mut adapter, &mut request);
    fs::write(
        request
            .original_common_dir
            .canonical_path
            .join("MERGE_HEAD"),
        "stale merge residue\n",
    )
    .expect("merge residue");

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(
        outcome,
        MergeExecutionOutcome::PreWriteRejected(PreWriteRejection::ExistingMergeResidue)
    );
    assert_eq!(
        git(root.path(), &["rev-parse", "HEAD"]),
        request.base_commit
    );
}

#[test]
fn commit_and_merge_returns_a_typed_conflict_without_following_writes() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved\n",
    )
    .expect("task change");
    approve_current_candidate(&mut adapter, &mut request);
    let observer = Arc::new(ConflictBeforeMerge {
        root: root.path().to_path_buf(),
        changed: AtomicBool::new(false),
    });
    adapter.set_write_command_observer(Some(observer.clone()));

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(outcome, MergeExecutionOutcome::ConfirmedMergeConflict);
    assert!(observer.changed.load(Ordering::SeqCst));
    assert!(
        request
            .original_common_dir
            .canonical_path
            .join("MERGE_HEAD")
            .exists()
    );
    assert_eq!(
        git(root.path(), &["rev-list", "--parents", "-n", "1", "HEAD"])
            .split_whitespace()
            .count(),
        2
    );
}

#[test]
fn commit_and_merge_treats_a_write_timeout_as_uncertain_without_staging() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved\n",
    )
    .expect("task change");
    approve_current_candidate(&mut adapter, &mut request);
    adapter.set_write_command_observer(Some(Arc::new(TimeoutBeforeStage)));

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(outcome, MergeExecutionOutcome::StageWriteUncertain);
    assert_eq!(
        git(root.path(), &["rev-parse", "HEAD"]),
        request.base_commit
    );
    assert_eq!(
        git(
            &request.task_worktree.canonical_path,
            &["rev-parse", "HEAD"]
        ),
        request.base_commit
    );
}

#[test]
fn commit_and_merge_rejects_an_autostash_residue_before_any_git_write() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved
",
    )
    .expect("task change");
    approve_current_candidate(&mut adapter, &mut request);
    let worktree_head_before = git(
        &request.task_worktree.canonical_path,
        &["rev-parse", "HEAD"],
    );
    fs::write(
        request
            .original_common_dir
            .canonical_path
            .join("MERGE_AUTOSTASH"),
        "0000000000000000000000000000000000000000
",
    )
    .expect("autostash residue");

    let outcome = adapter.commit_and_merge(&request);

    assert_eq!(
        outcome,
        MergeExecutionOutcome::PreWriteRejected(PreWriteRejection::ExistingMergeResidue),
        "a leftover MERGE_AUTOSTASH is merge residue exactly like MERGE_HEAD"
    );
    assert_eq!(
        git(root.path(), &["rev-parse", "HEAD"]),
        request.base_commit,
        "the base branch must not have moved"
    );
    assert_eq!(
        git(
            &request.task_worktree.canonical_path,
            &["rev-parse", "HEAD"]
        ),
        worktree_head_before,
        "no task commit may be created"
    );
    assert!(
        !git(
            &request.task_worktree.canonical_path,
            &["status", "--porcelain=v1"]
        )
        .is_empty(),
        "the approved change is still uncommitted in the task worktree, i.e.          `git add` never ran"
    );
}

#[test]
fn commit_and_merge_does_not_report_success_when_the_merge_lands_off_the_base_branch() {
    let (root, _worktree_parent, _control, mut adapter, mut request) = prepared_task();
    fs::write(
        request.task_worktree.canonical_path.join("tracked.txt"),
        "approved
",
    )
    .expect("task change");
    approve_current_candidate(&mut adapter, &mut request);
    let observer = Arc::new(DetachHeadBeforeMerge {
        root: root.path().to_path_buf(),
        detached: AtomicBool::new(false),
    });
    adapter.set_write_command_observer(Some(observer.clone()));

    let outcome = adapter.commit_and_merge(&request);

    adapter.set_write_command_observer(None);
    assert!(
        observer.detached.load(Ordering::SeqCst),
        "the fixture must actually have detached HEAD before the merge"
    );
    assert_eq!(
        outcome,
        MergeExecutionOutcome::PostWriteUncertain,
        "a merge that produced the right two parents but landed on a detached HEAD          instead of the expected base branch must not be reported as Merged"
    );
    assert_eq!(
        git(root.path(), &["rev-parse", "main"]),
        request.base_commit,
        "the expected base branch itself never moved, which is exactly why          the parent list alone is not enough evidence"
    );
}
