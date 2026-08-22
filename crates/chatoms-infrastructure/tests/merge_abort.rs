use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chatoms_domain::{ProjectId, TaskId};
use chatoms_infrastructure::git::{GitCliAdapter, GitWriteCommand, GitWriteCommandObserver};
use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    git::GitService,
    merge_abort::{
        MergeAbortOutcome, MergeAbortPort, MergeAbortPreWriteRejection, MergeAbortRequest,
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
    // The adapter's controlled environment never sets `core.autocrlf` (its
    // isolated global config is empty and `GIT_CONFIG_NOSYSTEM=1`), so it
    // always compares working-tree bytes literally. On a machine whose
    // ambient global config has `core.autocrlf=true`, this fixture's own
    // plain `git` calls (`reset --hard`, `checkout`, worktree creation)
    // would otherwise smudge LF to CRLF on checkout, making the adapter see
    // a spurious modification the ambient `git status` does not.
    git(directory.path(), &["config", "core.autocrlf", "false"]);
    fs::write(directory.path().join("tracked.txt"), "base\n").expect("base file");
    git(directory.path(), &["add", "tracked.txt"]);
    git(directory.path(), &["commit", "-qm", "test: base"]);
    directory
}

fn git_update_index_info(root: &Path, entries: &str) {
    use std::io::Write as _;
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
    // `git add` re-reads and re-hashes the file it just wrote, which
    // refreshes the index's cached stat info for the new content. Manually
    // injecting a pre-computed blob via `update-index --cacheinfo` instead
    // leaves that stat info stale/absent, which made `git merge --abort`'s
    // internal `unpack_trees` uptodate check flaky under this fixture's
    // rapid write-then-git-command timing (observed empirically on
    // Windows).
    git(root, &["add", "--", "tracked.txt"]);
}

struct Fixture {
    root: tempfile::TempDir,
    _worktree_parent: tempfile::TempDir,
    _control: tempfile::TempDir,
    adapter: GitCliAdapter,
    request: MergeAbortRequest,
}

fn prepared_conflict() -> Fixture {
    let root = repository();
    let worktree_parent = tempfile::tempdir().expect("worktree parent");
    let worktree = worktree_parent.path().join("task-worktree");
    let control = tempfile::tempdir().expect("Git control root");
    let mut adapter = GitCliAdapter::new(control.path().to_path_buf()).expect("Git adapter");
    let base_commit = git(root.path(), &["rev-parse", "HEAD"]);
    let safety = adapter
        .validate_repository_source(root.path(), &base_commit)
        .expect("repository safety evidence");
    let task_branch = "ai-task/merge-abort".to_owned();
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
    // A genuine conflicted merge never leaves a stage-0 entry alongside
    // stages 1-3 for the same path; without removing it first, `git merge
    // --abort`'s internal reset refuses with "not uptodate" even though the
    // working tree file content is untouched.
    git(root.path(), &["rm", "--cached", "-q", "--", "tracked.txt"]);
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
    fs::write(
        root.path().join(".git").join("MERGE_MSG"),
        "test: merge task branch\n",
    )
    .expect("merge message");
    fs::write(root.path().join(".git").join("MERGE_MODE"), "no-ff").expect("merge mode");

    let request = MergeAbortRequest {
        original_checkout: supported_directory_identity(root.path()).expect("root identity"),
        original_common_dir: supported_directory_identity(&root.path().join(".git"))
            .expect("common identity"),
        task_worktree: supported_directory_identity(&worktree).expect("worktree identity"),
        project_id: ProjectId::new(),
        task_id: TaskId::new(),
        merge_conflict_task_version: 3,
        source_approval_task_version: 1,
        base_branch: "main".to_owned(),
        task_branch,
        base_commit: base_commit.clone(),
        task_commit: task_commit.clone(),
        merge_head_commit: task_commit,
    };
    Fixture {
        root,
        _worktree_parent: worktree_parent,
        _control: control,
        adapter,
        request,
    }
}

fn assert_fully_restored(fixture: &Fixture) {
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
    let status = git(fixture.root.path(), &["status", "--porcelain=v1"]);
    assert!(status.is_empty(), "original checkout must be clean");
    for name in ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE", "MERGE_AUTOSTASH"] {
        assert!(
            !fixture.root.path().join(".git").join(name).exists(),
            "{name} must be gone after a successful abort"
        );
    }
    assert_eq!(
        git(
            &fixture.request.task_worktree.canonical_path,
            &["rev-parse", "HEAD"]
        ),
        fixture.request.task_commit,
        "task worktree HEAD must be unchanged"
    );
    assert_eq!(
        git(
            &fixture.request.task_worktree.canonical_path,
            &["symbolic-ref", "--quiet", "--short", "HEAD"]
        ),
        fixture.request.task_branch,
        "task worktree branch must be unchanged"
    );
}

#[test]
fn unresolved_conflict_is_aborted_and_fully_restores_base_state() {
    let mut fixture = prepared_conflict();

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(outcome, MergeAbortOutcome::Aborted);
    assert_fully_restored(&fixture);
}

#[test]
fn resolved_pending_confirmation_conflict_is_also_aborted() {
    let mut fixture = prepared_conflict();
    resolve_with_content(fixture.root.path(), "resolved\n");

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(outcome, MergeAbortOutcome::Aborted);
    assert_fully_restored(&fixture);
}

#[test]
fn autostash_present_is_rejected_without_any_write() {
    let mut fixture = prepared_conflict();
    fs::write(
        fixture.root.path().join(".git").join("MERGE_AUTOSTASH"),
        "deadbeef\n",
    )
    .expect("autostash marker");

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(
        outcome,
        MergeAbortOutcome::PreWriteRejected(MergeAbortPreWriteRejection::AutostashPresent)
    );
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
    assert!(fixture.root.path().join(".git").join("MERGE_HEAD").exists());
    assert!(
        fixture
            .root
            .path()
            .join(".git")
            .join("MERGE_AUTOSTASH")
            .exists(),
        "the unapproved autostash entry must not be touched"
    );
}

#[test]
fn foreign_operation_residue_is_rejected_without_any_write() {
    let mut fixture = prepared_conflict();
    fs::write(
        fixture.root.path().join(".git").join("CHERRY_PICK_HEAD"),
        format!("{}\n", fixture.request.base_commit),
    )
    .expect("foreign residue marker");

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(
        outcome,
        MergeAbortOutcome::PreWriteRejected(MergeAbortPreWriteRejection::ForeignOperationResidue)
    );
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
    assert!(fixture.root.path().join(".git").join("MERGE_HEAD").exists());
}

#[test]
fn identity_mismatch_is_rejected_before_any_write() {
    let mut fixture = prepared_conflict();
    fixture.request.task_worktree.file_id_hex = "0000000000000000".to_owned();

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(
        outcome,
        MergeAbortOutcome::PreWriteRejected(MergeAbortPreWriteRejection::IdentityOrTopology)
    );
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
}

#[test]
fn stale_approval_with_wrong_task_commit_is_rejected_before_any_write() {
    let mut fixture = prepared_conflict();
    fixture.request.task_commit = "a".repeat(40);
    fixture.request.merge_head_commit = "a".repeat(40);

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(
        outcome,
        MergeAbortOutcome::PreWriteRejected(MergeAbortPreWriteRejection::MergeIdentityMismatch)
    );
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
    assert!(fixture.root.path().join(".git").join("MERGE_HEAD").exists());
}

#[test]
fn already_restored_repository_is_confirmed_not_in_merge_without_any_write() {
    let fixture = prepared_conflict();
    // Simulate an earlier abort attempt that actually succeeded in Git but
    // whose SQLite commit never landed: manually restore the original
    // checkout to exactly the state a real `git merge --abort` would leave
    // it in, without ever invoking the adapter.
    for name in ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE"] {
        let path = fixture.root.path().join(".git").join(name);
        if path.exists() {
            fs::remove_file(path).expect("remove merge residue");
        }
    }
    git(fixture.root.path(), &["reset", "--hard", "HEAD"]);
    git(fixture.root.path(), &["clean", "-fd"]);
    let mut adapter = fixture.adapter;

    let outcome = adapter.abort_merge(&fixture.request);

    assert_eq!(outcome, MergeAbortOutcome::ConfirmedNotInMerge);
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
}

#[test]
fn a_merge_that_actually_landed_is_never_reported_as_confirmed_not_in_merge() {
    let fixture = prepared_conflict();
    // The merge actually landed (root HEAD moved past base_commit) --
    // deliberately distinct from "already restored to base_commit".
    for name in ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE"] {
        let path = fixture.root.path().join(".git").join(name);
        if path.exists() {
            fs::remove_file(path).expect("remove merge residue");
        }
    }
    fs::write(fixture.root.path().join("tracked.txt"), "landed\n").expect("landed file");
    git(fixture.root.path(), &["add", "tracked.txt"]);
    git(
        fixture.root.path(),
        &["commit", "-qm", "test: landed merge"],
    );
    let mut adapter = fixture.adapter;

    let outcome = adapter.abort_merge(&fixture.request);

    assert_eq!(
        outcome,
        MergeAbortOutcome::PreWriteRejected(MergeAbortPreWriteRejection::NotInMergeAndNotRestored)
    );
    assert_ne!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit,
        "the landed commit must not have been touched"
    );
}

struct CompetingWorktreeChangeBeforeAbort {
    worktree: std::path::PathBuf,
    changed: AtomicBool,
}

impl GitWriteCommandObserver for CompetingWorktreeChangeBeforeAbort {
    fn before_command(&self, command: GitWriteCommand) {
        if command != GitWriteCommand::MergeAbort || self.changed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Introduce a change to the task worktree between the pre-write
        // gate and the write itself, so the write can still succeed while
        // the restoration postcondition's task-worktree check fails
        // afterward.
        fs::write(self.worktree.join("tracked.txt"), "competing\n").expect("competing change");
        git(&self.worktree, &["add", "tracked.txt"]);
        git(&self.worktree, &["commit", "-qm", "test: competing change"]);
    }
}

#[test]
fn a_task_worktree_change_racing_the_write_is_reported_as_uncertain_not_aborted() {
    let mut fixture = prepared_conflict();
    let observer = Arc::new(CompetingWorktreeChangeBeforeAbort {
        worktree: fixture.request.task_worktree.canonical_path.clone(),
        changed: AtomicBool::new(false),
    });
    fixture.adapter.set_write_command_observer(Some(observer));

    let outcome = fixture.adapter.abort_merge(&fixture.request);

    assert_eq!(outcome, MergeAbortOutcome::PostWriteUncertain);
    // The Git write itself still succeeded and the original checkout is
    // genuinely restored -- only the task worktree's postcondition failed,
    // so this must never be reported as a confirmed `Aborted`.
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.request.base_commit
    );
}
