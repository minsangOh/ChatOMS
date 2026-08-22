use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use chatoms_domain::{ProjectId, TaskId};
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    git::GitService,
    manual_merge_resolution::{
        ManualMergeResolutionCandidatePort, ManualMergeResolutionCandidateRequest,
        ManualResolutionCandidateOutcome,
    },
    merge_continue::{MergeContinueOutcome, MergeContinuePort, MergeContinueRequest},
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

struct Fixture {
    root: tempfile::TempDir,
    _worktree_parent: tempfile::TempDir,
    _control: tempfile::TempDir,
    adapter: GitCliAdapter,
    candidate_request: ManualMergeResolutionCandidateRequest,
    base_commit: String,
    task_commit: String,
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
    let task_branch = "ai-task/merge-continue".to_owned();
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
    // A real conflicted merge always leaves MERGE_MSG behind; without it,
    // `git commit` (invoked internally by `merge --continue`) has no
    // prepared message and falls back to launching an editor, which hangs
    // this fixture's null stdin instead of failing fast.
    fs::write(
        root.path().join(".git").join("MERGE_MSG"),
        "test: merge task branch\n",
    )
    .expect("merge message");
    // Production conflicts always originate from the adapter's first merge
    // attempt, which always passes `--no-ff` (see `merge_execution.rs`), so
    // MERGE_MODE always exists by the time a real MergeConflict occurs.
    // Without it here, `base_commit` being an ancestor of `task_commit`
    // makes `merge --continue` fast-forward-collapse the result into a
    // single-parent commit instead of a real two-parent merge commit —
    // confirmed empirically.
    fs::write(root.path().join(".git").join("MERGE_MODE"), "no-ff").expect("merge mode");
    let candidate_request = ManualMergeResolutionCandidateRequest {
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
        base_commit: base_commit.clone(),
    };
    Fixture {
        root,
        _worktree_parent: worktree_parent,
        _control: control,
        adapter,
        candidate_request,
        base_commit,
        task_commit,
    }
}

fn merge_continue_request(
    fixture: &mut Fixture,
    confirmed_digest_from_live: bool,
) -> MergeContinueRequest {
    let outcome = fixture
        .adapter
        .resolution_candidate(&fixture.candidate_request);
    let digest = match outcome {
        ManualResolutionCandidateOutcome::Ready(candidate) if confirmed_digest_from_live => {
            candidate.resolution_digest
        }
        _ => chatoms_ports::manual_merge_resolution::ManualResolutionDigest::from_digest_bytes(
            [0xAB; 32],
        ),
    };
    MergeContinueRequest {
        original_checkout: fixture.candidate_request.original_checkout.clone(),
        original_common_dir: fixture.candidate_request.original_common_dir.clone(),
        task_worktree: fixture.candidate_request.task_worktree.clone(),
        project_id: fixture.candidate_request.project_id,
        task_id: fixture.candidate_request.task_id,
        merge_conflict_task_version: fixture.candidate_request.merge_conflict_task_version,
        source_approval_task_version: fixture.candidate_request.source_approval_task_version,
        base_branch: fixture.candidate_request.base_branch.clone(),
        task_branch: fixture.candidate_request.task_branch.clone(),
        base_commit: fixture.base_commit.clone(),
        task_commit: fixture.task_commit.clone(),
        merge_head_commit: fixture.task_commit.clone(),
        confirmed_resolution_digest: digest,
    }
}

#[test]
fn continued_creates_the_exact_two_parent_commit_and_clears_residue() {
    let mut fixture = prepared_conflict();
    resolve_with_content(fixture.root.path(), "resolved\n");
    let request = merge_continue_request(&mut fixture, true);

    let outcome = fixture.adapter.continue_merge(&request);

    assert_eq!(outcome, MergeContinueOutcome::Continued);
    let new_head = git(fixture.root.path(), &["rev-parse", "HEAD"]);
    let parents = git(
        fixture.root.path(),
        &["rev-list", "--parents", "-n", "1", "HEAD"],
    );
    let parts: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], new_head);
    assert_eq!(parts[1], fixture.base_commit);
    assert_eq!(parts[2], fixture.task_commit);
    assert!(!fixture.root.path().join(".git").join("MERGE_HEAD").exists());
    let status = git(fixture.root.path(), &["status", "--porcelain=v1"]);
    assert!(status.is_empty(), "repository must be clean after continue");
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("tracked.txt")).expect("merged file"),
        "resolved\n"
    );
}

#[test]
fn stale_confirmation_is_rejected_without_writing() {
    let mut fixture = prepared_conflict();
    resolve_with_content(fixture.root.path(), "resolved\n");
    // A digest that does not match the currently-staged resolution.
    let mut request = merge_continue_request(&mut fixture, true);
    request.confirmed_resolution_digest =
        chatoms_ports::manual_merge_resolution::ManualResolutionDigest::from_digest_bytes(
            [0x11; 32],
        );

    let outcome = fixture.adapter.continue_merge(&request);

    assert_eq!(outcome, MergeContinueOutcome::ConfirmationStale);
    assert!(fixture.root.path().join(".git").join("MERGE_HEAD").exists());
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.base_commit
    );
}

#[test]
fn unresolved_conflict_reports_confirmed_pending_without_writing() {
    let mut fixture = prepared_conflict();
    // Deliberately left unresolved.
    let request = merge_continue_request(&mut fixture, false);

    let outcome = fixture.adapter.continue_merge(&request);

    assert_eq!(outcome, MergeContinueOutcome::ConfirmedMergePending);
    assert!(fixture.root.path().join(".git").join("MERGE_HEAD").exists());
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.base_commit
    );
}

#[test]
fn identity_mismatch_is_rejected_before_any_write() {
    let mut fixture = prepared_conflict();
    resolve_with_content(fixture.root.path(), "resolved\n");
    let mut request = merge_continue_request(&mut fixture, true);
    request.task_worktree.file_id_hex = "0000000000000000".to_owned();

    let outcome = fixture.adapter.continue_merge(&request);

    assert_eq!(outcome, MergeContinueOutcome::PreWriteRejected);
    assert_eq!(
        git(fixture.root.path(), &["rev-parse", "HEAD"]),
        fixture.base_commit
    );
}

// A "no author identity available" case is deliberately not covered here:
// the adapter's author lookup falls back to the *real machine's* global Git
// config (mirroring `GitService::has_commit_author`'s existing production
// behavior), so a test asserting its absence would depend on ambient
// developer-machine state rather than this fixture's isolated repository —
// exactly the kind of environment-dependent flakiness that caused this Unit
// to hang during development. `identity_mismatch_is_rejected_before_any_write`
// already covers the `PreWriteRejected` outcome without any write.
