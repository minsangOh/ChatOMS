mod support;

use std::path::Path;

use chatoms_application::{
    error::ApplicationErrorCode,
    user_diff_approval::{
        ApproveUserDiffRequest, ReadUserDiffForReviewRequest, UserDiffApprovalService,
        UserDiffReviewReader, hash_diff_text,
    },
};
use chatoms_domain::{ProjectId, TaskId, TaskState};
use chatoms_ports::{
    diff::{CommitCandidate, CommitCandidateOutcome, CommitCandidatePort},
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    repository::{GitIsolationStatus, ProjectRecord, TaskGitIsolation},
};

use support::{FakeRepository, FakeTime, restored_task};

/// Only `inspect_supported_directory`/`verify_local_tree` are exercised;
/// `acquire_guard` panics if called since this reader never mutates
/// anything and so never needs a mutation guard.
struct FilesystemFake {
    verify_ok: bool,
}

impl FilesystemIdentityPort for FilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000001".to_owned(),
            file_id_hex: "00000000000000000000000000000001".to_owned(),
        })
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        if self.verify_ok {
            Ok(())
        } else {
            Err(PortFailure::new(FailureCategory::Conflict))
        }
    }

    fn acquire_guard(
        &mut self,
        _path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        unreachable!("not used by UserDiffReviewReader")
    }
}

/// Records candidate reads and returns one scripted read-only disposition.
struct DiffFake {
    called: usize,
    result: Option<Result<CommitCandidateOutcome, PortFailure>>,
}

impl DiffFake {
    fn scripted(result: Result<CommitCandidateOutcome, PortFailure>) -> Self {
        Self {
            called: 0,
            result: Some(result),
        }
    }
}

impl CommitCandidatePort for DiffFake {
    fn current_commit_candidate(
        &mut self,
        _root: &Path,
        _base_branch: &str,
        _task_branch: &str,
        _base_commit: &str,
        _worktree: &Path,
    ) -> Result<CommitCandidateOutcome, PortFailure> {
        self.called += 1;
        self.result
            .take()
            .expect("candidate read called at most once in these tests")
    }
}

const ROOT_PATH: &str = "C:/projects/example";
const WORKTREE_PATH: &str = "C:/managed/task";

fn project_record(project_id: ProjectId) -> ProjectRecord {
    ProjectRecord {
        id: project_id,
        name: "Example".to_owned(),
        root_path: ROOT_PATH.to_owned(),
        canonical_path_key: ROOT_PATH.to_ascii_lowercase(),
        display_path: ROOT_PATH.to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn worktree_ready_isolation(
    task_id: TaskId,
    project_id: ProjectId,
    version: u64,
) -> TaskGitIsolation {
    TaskGitIsolation {
        task_id,
        project_id,
        status: GitIsolationStatus::WorktreeReady,
        operation_id: None,
        expected_task_version: version,
        base_branch: Some("main".to_owned()),
        base_commit: Some("a".repeat(40)),
        worktree_path: Some(WORKTREE_PATH.to_owned()),
        branch_created_by_app: true,
        worktree_created_by_app: true,
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

/// A task in `AwaitingUserDiffApproval` with a matching `WorktreeReady`
/// isolation record and an owning project, all consistent with each other —
/// ready for `UserDiffReviewReader`/`UserDiffApprovalService` to succeed.
fn setup_awaiting_diff_approval(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, version, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository.isolations.insert(
        task_id,
        worktree_ready_isolation(task_id, project_id, version),
    );
    repository.seed_task(task, history);
    (repository, task_id)
}

fn diff_of(text: &str) -> DiffFake {
    let hash = hash_diff_text(text);
    DiffFake::scripted(Ok(CommitCandidateOutcome::Candidate(CommitCandidate::new(
        text.to_owned(),
        hash,
    ))))
}

#[test]
fn read_diff_for_review_returns_the_transient_diff_and_its_hash_without_any_mutation() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("diff --git a/x b/x\n+line\n");

    let before_task = repository.tasks.get(&task_id).cloned();
    let before_history_len = repository.transitions.get(&task_id).map(Vec::len);
    let before_lease = repository.active_lease;

    let review = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
        .expect("read succeeds when every precondition holds");

    assert_eq!(review.diff_text(), "diff --git a/x b/x\n+line\n");
    assert_eq!(
        review.diff_content_hash,
        hash_diff_text("diff --git a/x b/x\n+line\n")
    );
    assert_eq!(repository.tasks.get(&task_id).cloned(), before_task);
    assert_eq!(
        repository.transitions.get(&task_id).map(Vec::len),
        before_history_len
    );
    assert_eq!(repository.active_lease, before_lease);
    assert!(repository.diff_approvals.is_empty());
}

#[test]
fn read_diff_for_review_debug_output_hides_the_diff_text() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("SECRET_LEAK_MARKER_must_never_appear_in_debug_output");

    let review = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
        .expect("read succeeds");

    let debug = format!("{review:?}");
    assert!(!debug.contains("SECRET_LEAK_MARKER_must_never_appear_in_debug_output"));
    assert!(debug.contains("byte_len"));
}

#[test]
fn read_diff_for_review_rejects_a_task_in_the_wrong_state() {
    let (task, history) = restored_task(TaskState::Reviewing, 3, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, project_id, 3));
    repository.seed_task(task, history);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("unused");

    let error = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
        .expect_err("a task not in AwaitingUserDiffApproval must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert_eq!(diff.called, 0, "a candidate read must never be started");
}

#[test]
fn read_diff_for_review_rejects_a_stale_version() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("unused");

    let error = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 2))
        .expect_err("a stale expected_version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(diff.called, 0, "a Git process must never be spawned");
}

#[test]
fn read_diff_for_review_rejects_a_missing_isolation_record() {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, 3, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository.seed_task(task, history);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("unused");

    let error = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
        .expect_err("a missing isolation record must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(diff.called, 0, "a Git process must never be spawned");
}

#[test]
fn read_diff_for_review_rejects_a_git_identity_mismatch_before_reading_any_diff() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = DiffFake::scripted(Err(PortFailure::new(FailureCategory::Conflict)));

    let error = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
        .expect_err("an unverified worktree must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert_eq!(diff.called, 1, "the candidate verifier must fail closed");
}

#[test]
fn read_diff_for_review_rejects_a_filesystem_identity_mismatch_before_reading_any_diff() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut filesystem = FilesystemFake { verify_ok: false };
    let mut diff = diff_of("unused");

    let error = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
        .expect_err("a filesystem identity mismatch must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert_eq!(diff.called, 0, "a Git process must never be spawned");
}

#[test]
fn read_diff_for_review_rejects_every_non_diff_outcome() {
    for outcome in [
        CommitCandidateOutcome::NoChanges,
        CommitCandidateOutcome::CandidateTooLarge,
        CommitCandidateOutcome::TimedOut,
        CommitCandidateOutcome::Uncertain,
    ] {
        let description = format!("{outcome:?} must never be treated as a usable diff");
        let (mut repository, task_id) = setup_awaiting_diff_approval(3);
        let mut filesystem = FilesystemFake { verify_ok: true };
        let mut diff = DiffFake::scripted(Ok(outcome));

        let error = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut diff)
            .read_diff_for_review(&ReadUserDiffForReviewRequest::new(task_id, 3))
            .expect_err(&description);

        assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    }
}

#[test]
fn approve_recomputes_the_hash_and_records_an_approval_on_an_exact_match() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut time = FakeTime::at(500);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("diff --git a/x b/x\n+line\n");
    let expected_hash = hash_diff_text("diff --git a/x b/x\n+line\n");

    let view = UserDiffApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut diff)
        .approve(ApproveUserDiffRequest::new(task_id, 3, expected_hash))
        .expect("an exact hash match must succeed");

    assert_eq!(view.task_id, task_id);
    assert_eq!(view.approved_task_version, 3);
    assert_eq!(view.diff_content_hash, expected_hash);
    assert_eq!(view.approved_at_ms, 500);
    assert_eq!(repository.diff_approvals.len(), 1);
}

#[test]
fn approve_reuses_an_existing_approval_for_the_same_exact_hash() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut time = FakeTime::at(500);
    let expected_hash = hash_diff_text("diff --git a/x b/x\n+line\n");

    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("diff --git a/x b/x\n+line\n");
    let first =
        UserDiffApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut diff)
            .approve(ApproveUserDiffRequest::new(task_id, 3, expected_hash))
            .expect("first approval succeeds");

    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("diff --git a/x b/x\n+line\n");
    let second =
        UserDiffApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut diff)
            .approve(ApproveUserDiffRequest::new(task_id, 3, expected_hash))
            .expect("second approval for the exact same diff reuses the row");

    assert_eq!(first, second);
    assert_eq!(repository.diff_approvals.len(), 1);
}

#[test]
fn approve_rejects_a_hash_mismatch_without_creating_any_approval_or_touching_task_state() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut time = FakeTime::at(500);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = diff_of("diff --git a/x b/x\n+line\n");
    let wrong_hash = hash_diff_text("a completely different diff");

    let before_task = repository.tasks.get(&task_id).cloned();

    let error =
        UserDiffApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut diff)
            .approve(ApproveUserDiffRequest::new(task_id, 3, wrong_hash))
            .expect_err("a mismatched hash must never be approved");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert!(repository.diff_approvals.is_empty());
    assert_eq!(repository.tasks.get(&task_id).cloned(), before_task);
    assert_eq!(
        before_task.map(|task| task.state()),
        Some(TaskState::AwaitingUserDiffApproval),
        "the task must never transition to Merging or Completed on a mismatch"
    );
}

#[test]
fn approve_rejects_every_non_diff_outcome_without_creating_any_approval() {
    for outcome in [
        CommitCandidateOutcome::NoChanges,
        CommitCandidateOutcome::CandidateTooLarge,
        CommitCandidateOutcome::TimedOut,
        CommitCandidateOutcome::Uncertain,
    ] {
        let description = format!("{outcome:?} must never be approved");
        let (mut repository, task_id) = setup_awaiting_diff_approval(3);
        let mut time = FakeTime::at(500);
        let mut filesystem = FilesystemFake { verify_ok: true };
        let mut diff = DiffFake::scripted(Ok(outcome));
        let any_hash = hash_diff_text("irrelevant");

        let error =
            UserDiffApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut diff)
                .approve(ApproveUserDiffRequest::new(task_id, 3, any_hash))
                .expect_err(&description);

        assert_eq!(error.code(), ApplicationErrorCode::Conflict);
        assert!(repository.diff_approvals.is_empty());
    }
}

#[test]
fn approve_rejects_a_git_identity_mismatch_before_ever_reading_a_diff() {
    let (mut repository, task_id) = setup_awaiting_diff_approval(3);
    let mut time = FakeTime::at(500);
    let mut filesystem = FilesystemFake { verify_ok: true };
    let mut diff = DiffFake::scripted(Err(PortFailure::new(FailureCategory::Conflict)));
    let any_hash = hash_diff_text("irrelevant");

    let error =
        UserDiffApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut diff)
            .approve(ApproveUserDiffRequest::new(task_id, 3, any_hash))
            .expect_err("an unverified worktree must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert_eq!(diff.called, 1, "the candidate verifier must fail closed");
    assert!(repository.diff_approvals.is_empty());
}

#[test]
fn hash_diff_text_is_deterministic_and_distinguishes_distinct_content() {
    let a = hash_diff_text("same text");
    let b = hash_diff_text("same text");
    let c = hash_diff_text("different text");
    assert_eq!(a, b);
    assert_ne!(a, c);
}
