mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    error::ApplicationErrorCode,
    review_diff::{ReadWorktreeDiffRequest, ReviewDiffReader},
};
use chatoms_domain::{ProjectId, TaskId, TaskState};
use chatoms_ports::{
    diff::{WorktreeDiff, WorktreeDiffOutcome, WorktreeDiffPort},
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
    repository::{GitIsolationStatus, ProjectRecord, TaskGitIsolation},
};

use support::{FakeRepository, restored_task};

/// Only `verify_task_worktree` is exercised by `ReviewDiffReader`; every
/// other `GitService` method panics if called, so a wrongly-ordered
/// implementation that spawns Git through some other path fails loudly
/// instead of silently.
struct GitFake {
    verified: bool,
    calls: usize,
}

impl GitService for GitFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn inspect_project(&mut self, _input: &Path) -> Result<ProjectInspection, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn repository_status(&mut self, _root: &Path) -> Result<RepositoryStatus, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn validate_non_git_source(&mut self, _root: &Path) -> Result<(), PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn validate_repository_source(
        &mut self,
        _root: &Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn initialize_repository(&mut self, _root: &Path) -> Result<(), PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn has_commit_author(&mut self, _root: &Path) -> Result<bool, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn create_initial_snapshot(&mut self, _root: &Path) -> Result<String, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn create_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        unreachable!("not used by ReviewDiffReader")
    }
    fn verify_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
    ) -> Result<bool, PortFailure> {
        self.calls += 1;
        Ok(self.verified)
    }
}

/// Only `inspect_supported_directory`/`verify_local_tree` are exercised;
/// `acquire_guard` panics if called since `ReviewDiffReader` never mutates
/// anything and so never needs a mutation guard.
struct FilesystemFake {
    verify_ok: bool,
    calls: usize,
}

impl FilesystemIdentityPort for FilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.calls += 1;
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
        unreachable!("not used by ReviewDiffReader")
    }
}

/// Records whether `current_diff` was ever called (so tests can assert a
/// rejected precondition never reaches it) and returns a scripted result
/// exactly once.
struct DiffFake {
    called_with: Option<PathBuf>,
    result: Option<Result<WorktreeDiffOutcome, PortFailure>>,
}

impl DiffFake {
    fn scripted(result: Result<WorktreeDiffOutcome, PortFailure>) -> Self {
        Self {
            called_with: None,
            result: Some(result),
        }
    }
}

impl WorktreeDiffPort for DiffFake {
    fn current_diff(&mut self, worktree: &Path) -> Result<WorktreeDiffOutcome, PortFailure> {
        self.called_with = Some(worktree.to_path_buf());
        self.result
            .take()
            .expect("current_diff called at most once in these tests")
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

/// A task in `Reviewing` with a matching `WorktreeReady` isolation record
/// and owning project, all consistent with each other.
fn setup_reviewing(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Reviewing, version, 20, None);
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

#[test]
fn read_current_diff_verifies_identity_then_returns_the_diff_port_outcome() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::Diff(WorktreeDiff::new(
        "--- a/f\n+++ b/f\n".to_owned(),
    ))));

    let outcome = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 3))
        .expect("read succeeds");

    let WorktreeDiffOutcome::Diff(text) = outcome else {
        panic!("expected a Diff outcome");
    };
    assert_eq!(text.text(), "--- a/f\n+++ b/f\n");
    assert_eq!(git.calls, 1);
    assert!(filesystem.calls >= 1);
    assert_eq!(diff.called_with, Some(PathBuf::from(WORKTREE_PATH)));
}

#[test]
fn non_diff_outcomes_pass_through_unchanged() {
    let (mut repository, task_id) = setup_reviewing(1);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::DiffTooLarge));

    let outcome = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect("read succeeds");

    assert_eq!(outcome, WorktreeDiffOutcome::DiffTooLarge);
}

#[test]
fn diff_port_error_propagates_as_an_application_error() {
    let (mut repository, task_id) = setup_reviewing(1);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Err(PortFailure::new(FailureCategory::InvalidInput)));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("malformed diff output errs");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
}

#[test]
fn git_identity_mismatch_is_rejected_before_the_diff_port_is_ever_called() {
    let (mut repository, task_id) = setup_reviewing(1);
    let mut git = GitFake {
        verified: false,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("unverified worktree identity errs");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert_eq!(git.calls, 1);
    assert_eq!(diff.called_with, None);
}

#[test]
fn filesystem_identity_mismatch_is_rejected_before_the_diff_port_is_ever_called() {
    let (mut repository, task_id) = setup_reviewing(1);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: false,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("unverified filesystem identity errs");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert_eq!(diff.called_with, None);
}

#[test]
fn wrong_task_state_is_rejected_without_touching_git_or_diff_ports() {
    let (task, history) = restored_task(TaskState::Testing, 1, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, project_id, 1));
    repository.seed_task(task, history);

    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("a Testing task is not reviewable yet");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert_eq!(git.calls, 0);
    assert_eq!(diff.called_with, None);
}

#[test]
fn stale_version_is_rejected() {
    let (mut repository, task_id) = setup_reviewing(2);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("stale expected_version errs");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(diff.called_with, None);
}

#[test]
fn missing_isolation_record_is_rejected() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository.seed_task(task, history);

    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("missing isolation record errs");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(diff.called_with, None);
}

#[test]
fn isolation_not_worktree_ready_is_rejected() {
    let (task, history) = restored_task(TaskState::Reviewing, 1, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    let mut isolation = worktree_ready_isolation(task_id, project_id, 1);
    isolation.status = GitIsolationStatus::RecoveryRequired;
    repository.isolations.insert(task_id, isolation);
    repository.seed_task(task, history);

    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewDiffReader::new(&mut repository, &mut git, &mut filesystem, &mut diff)
        .read_current_diff(&ReadWorktreeDiffRequest::new(task_id, 1))
        .expect_err("a non-WorktreeReady isolation record errs");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert_eq!(git.calls, 0);
    assert_eq!(diff.called_with, None);
}
