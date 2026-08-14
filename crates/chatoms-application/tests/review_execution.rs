mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    error::ApplicationErrorCode,
    review_execution::{
        BeginReviewExecutionRequest, ReviewExecutionInputs, ReviewExecutionRecorder,
        ReviewExecutionStarter,
    },
};
use chatoms_domain::{ProjectId, TaskId, TaskState, WorkKind};
use chatoms_ports::{
    diff::{WorktreeDiff, WorktreeDiffOutcome, WorktreeDiffPort},
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
    process::{AtomicCancellationSignal, CancellationSignal},
    provider::{
        ProviderCapabilities, ProviderCapabilityPort, ProviderCapabilityStatus, ProviderKind,
    },
    repository::{
        FoundationRepository, GitIsolationStatus, ProjectRecord, ReviewResultOutcome,
        TaskBriefRecord, TaskGitIsolation,
    },
    review::{
        ClaudeReviewExecutor, ReviewExecutionBrief, ReviewExecutionResult,
        ReviewExecutionStartOutcome,
    },
};

use support::{FakeRepository, FakeTime, restored_task};

struct FakeCapability(ProviderCapabilityStatus);

impl ProviderCapabilityPort for FakeCapability {
    fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure> {
        Ok(ProviderCapabilities {
            claude: self.0,
            codex: ProviderCapabilityStatus::Unsupported,
        })
    }
}

/// Only `verify_task_worktree` is exercised by `ReviewDiffReader` (which
/// `ReviewExecutionStarter::begin` delegates to); every other `GitService`
/// method panics if called, so a wrongly-ordered implementation that spawns
/// Git through some other path fails loudly instead of silently.
struct GitFake {
    verified: bool,
    calls: usize,
}

impl GitService for GitFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn inspect_project(&mut self, _input: &Path) -> Result<ProjectInspection, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn repository_status(&mut self, _root: &Path) -> Result<RepositoryStatus, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn validate_non_git_source(&mut self, _root: &Path) -> Result<(), PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn validate_repository_source(
        &mut self,
        _root: &Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn initialize_repository(&mut self, _root: &Path) -> Result<(), PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn has_commit_author(&mut self, _root: &Path) -> Result<bool, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn create_initial_snapshot(&mut self, _root: &Path) -> Result<String, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
    }
    fn create_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        unreachable!("not used by ReviewExecutionStarter")
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
/// `acquire_guard` panics if called since Review never mutates anything and
/// so never needs a mutation guard.
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
        unreachable!("not used by ReviewExecutionStarter")
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

/// Deliberately looks like a leaked credential so panic-containment tests
/// can assert this exact string never survives into anything the caught
/// panic's containment path returns, records, or renders.
const PANIC_SENTINEL: &str = "SIMULATED_EXECUTOR_PANIC_must_never_leak_sk-fake0000000000";

type ObservedRun = (PathBuf, String, String, String, String);

struct ScriptedExecutor {
    scripted: Option<Result<ReviewExecutionStartOutcome, ()>>,
    observed: Vec<ObservedRun>,
    panics: bool,
}

impl ScriptedExecutor {
    fn completed(outcome: ReviewResultOutcome, review_text: Option<&str>) -> Self {
        Self {
            scripted: Some(Ok(ReviewExecutionStartOutcome::Completed(
                ReviewExecutionResult {
                    outcome,
                    exit_code: Some(0),
                    turn_count: Some(1),
                    review_text: review_text.map(ToOwned::to_owned),
                },
            ))),
            observed: Vec::new(),
            panics: false,
        }
    }

    fn preflight_rejected() -> Self {
        Self {
            scripted: Some(Ok(ReviewExecutionStartOutcome::PreflightRejected)),
            observed: Vec::new(),
            panics: false,
        }
    }

    fn failing() -> Self {
        Self {
            scripted: Some(Err(())),
            observed: Vec::new(),
            panics: false,
        }
    }

    /// Simulates a genuine Rust panic inside the executor (e.g. an
    /// unexpected crash deep in adapter/process-runner code), rather than an
    /// ordinary `Err`.
    fn panicking() -> Self {
        Self {
            scripted: None,
            observed: Vec::new(),
            panics: true,
        }
    }
}

impl ClaudeReviewExecutor for ScriptedExecutor {
    fn start_review(
        &mut self,
        worktree: &Path,
        brief: ReviewExecutionBrief<'_>,
        _cancellation: &dyn CancellationSignal,
    ) -> Result<ReviewExecutionStartOutcome, PortFailure> {
        self.observed.push((
            worktree.to_path_buf(),
            brief.requirements.to_owned(),
            brief.completion_criteria.to_owned(),
            brief.prohibited_scope.to_owned(),
            brief.diff_text.to_owned(),
        ));
        if self.panics {
            panic!("{PANIC_SENTINEL}");
        }
        match self.scripted.take() {
            Some(Ok(outcome)) => Ok(outcome),
            Some(Err(())) | None => Err(PortFailure::new(FailureCategory::Unsupported)),
        }
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

fn brief_record(task_id: TaskId) -> TaskBriefRecord {
    TaskBriefRecord {
        task_id,
        requirements: "Add CSV export".to_owned(),
        completion_criteria: "Export button downloads a CSV".to_owned(),
        prohibited_scope: "Do not touch the import pipeline".to_owned(),
        created_at_ms: 10,
    }
}

/// A task in `Reviewing` with a matching `WorktreeReady` isolation record, an
/// owning project, and an attached `TaskBrief`, all consistent with each
/// other — ready for `ReviewExecutionStarter::begin` to succeed.
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
    repository.briefs.insert(task_id, brief_record(task_id));
    repository.seed_task(task, history);
    (repository, task_id)
}

/// A task already in `Reviewing`, with no isolation/brief evidence attached
/// — sufficient for `ReviewExecutionRecorder`, which never reads either.
fn setup_reviewing_only(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Reviewing, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    (repository, task_id)
}

fn diff_of(text: &str) -> DiffFake {
    DiffFake::scripted(Ok(WorktreeDiffOutcome::Diff(WorktreeDiff::new(
        text.to_owned(),
    ))))
}

#[test]
fn begin_records_consent_and_returns_worktree_brief_and_diff_when_every_precondition_holds() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("--- a/f\n+++ b/f\n");

    let inputs = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect("begin succeeds");

    assert_eq!(inputs.task.state, TaskState::Reviewing);
    assert_eq!(
        inputs.task.version, 3,
        "starting Review drives no state transition, so the version is unchanged"
    );
    assert_eq!(inputs.worktree_path, WORKTREE_PATH);
    assert_eq!(inputs.brief.requirements, "Add CSV export");
    assert_eq!(inputs.diff_text, "--- a/f\n+++ b/f\n");
    assert_eq!(git.calls, 1);
    assert!(filesystem.calls >= 1);

    let consent = repository
        .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
        .expect("consent lookup")
        .expect("consent recorded exactly once by begin");
    assert_eq!(consent.approved_task_version, 3);
}

#[test]
fn begin_reuses_an_existing_same_version_review_consent() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut time = FakeTime::at(30);

    {
        let mut git = GitFake {
            verified: true,
            calls: 0,
        };
        let mut filesystem = FilesystemFake {
            verify_ok: true,
            calls: 0,
        };
        let mut diff = diff_of("diff-1");
        ReviewExecutionStarter::new(
            &mut repository,
            &mut time,
            &mut capability,
            &mut git,
            &mut filesystem,
            &mut diff,
        )
        .begin(BeginReviewExecutionRequest::new(task_id, 3))
        .expect("first begin succeeds");
    }
    let first_consent = repository
        .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
        .expect("consent lookup")
        .expect("consent recorded");

    time.now = 999;

    {
        let mut git = GitFake {
            verified: true,
            calls: 0,
        };
        let mut filesystem = FilesystemFake {
            verify_ok: true,
            calls: 0,
        };
        let mut diff = diff_of("diff-2");
        ReviewExecutionStarter::new(
            &mut repository,
            &mut time,
            &mut capability,
            &mut git,
            &mut filesystem,
            &mut diff,
        )
        .begin(BeginReviewExecutionRequest::new(task_id, 3))
        .expect("second begin also succeeds, reusing the consent");
    }
    let second_consent = repository
        .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
        .expect("consent lookup")
        .expect("consent recorded");

    assert_eq!(
        first_consent.consented_at_ms, second_consent.consented_at_ms,
        "consent must be reused, not recreated with a new timestamp"
    );
}

#[test]
fn begin_rejects_unsupported_capability_with_no_execution_and_state_preserved() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Unsupported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("unused");

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("unsupported capability must reject before any read or write");

    assert_eq!(error.code(), ApplicationErrorCode::Unsupported);
    assert_eq!(git.calls, 0);
    assert_eq!(diff.called_with, None);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
    assert_eq!(repository.tasks[&task_id].version(), 3);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_when_task_is_not_reviewing() {
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
    repository.briefs.insert(task_id, brief_record(task_id));
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("unused");

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 1))
    .expect_err("a Testing task cannot start Review");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert_eq!(diff.called_with, None);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 1)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_a_stale_task_version() {
    let (mut repository, task_id) = setup_reviewing(5);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("unused");

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 1))
    .expect_err("stale expected_version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(diff.called_with, None);
}

#[test]
fn begin_rejects_missing_isolation_record_with_no_state_change() {
    let (task, history) = restored_task(TaskState::Reviewing, 3, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository.briefs.insert(task_id, brief_record(task_id));
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("unused");

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("begin requires a WorktreeReady isolation record");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(diff.called_with, None);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_missing_brief_with_no_state_change() {
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
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("unused");

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("begin requires a TaskBrief");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(diff.called_with, None);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_a_no_changes_diff_with_no_consent_or_state_change() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::NoChanges));

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("an empty diff has nothing to review");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Reviewing);
}

#[test]
fn begin_rejects_a_too_large_diff_with_no_consent_or_state_change() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::DiffTooLarge));

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("an oversized diff must not start a Review run");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_a_timed_out_diff_read_with_no_consent_or_state_change() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::TimedOut));

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("a timed-out diff read must not start a Review run");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_an_uncertain_diff_read_with_no_consent_or_state_change() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Ok(WorktreeDiffOutcome::Uncertain));

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("an unconfirmed diff read must not start a Review run");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_a_git_identity_mismatch_before_the_diff_port_is_ever_called() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: false,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("unused");

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("an unverified worktree identity must not start a Review run");

    assert_eq!(error.code(), ApplicationErrorCode::Conflict);
    assert_eq!(diff.called_with, None);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_rejects_a_malformed_diff_port_error_with_no_consent_or_state_change() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = DiffFake::scripted(Err(PortFailure::new(FailureCategory::InvalidInput)));

    let error = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect_err("a genuine Git/diff-port failure must not start a Review run");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(
        repository
            .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn review_execution_inputs_debug_output_never_reveals_the_diff_text() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let secret_diff =
        "diff --git a/config.json b/config.json\n+api_key=sk-should-not-leak-in-debug";
    let mut diff = diff_of(secret_diff);

    let inputs = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect("begin succeeds");

    let rendered = format!("{inputs:?}");
    assert!(!rendered.contains("sk-should-not-leak-in-debug"));
    assert!(rendered.contains("diff_text_byte_len"));
}

#[test]
fn run_and_record_completed_reaches_awaiting_user_diff_approval() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor =
        ScriptedExecutor::completed(ReviewResultOutcome::Completed, Some("masked review text"));
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::AwaitingUserDiffApproval);
    assert!(
        repository.active_lease.is_some(),
        "AwaitingUserDiffApproval still requires the active lease"
    );
    let stored = repository
        .review_results
        .get(&task_id)
        .expect("a review result row was recorded");
    assert_eq!(stored.review_text.as_deref(), Some("masked review text"));
    assert_eq!(executor.observed[0].0, Path::new(WORKTREE_PATH));
    assert_eq!(executor.observed[0].4, "diff --git a/f b/f\n");
}

#[test]
fn run_and_record_failed_reaches_failed_and_releases_the_lease() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::completed(ReviewResultOutcome::Failed, None);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::Failed);
    assert!(
        repository.active_lease.is_none(),
        "Failed is terminal for Review and must release the lease"
    );
}

#[test]
fn run_and_record_confirmed_cancel_reaches_paused_with_reviewing_resume_target() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::completed(ReviewResultOutcome::Cancelled, None);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a confirmed cancellation is recorded");

    assert_eq!(view.state, TaskState::Paused);
    assert_eq!(view.resume_target_state, Some(TaskState::Reviewing));
    assert!(
        repository.active_lease.is_some(),
        "Paused must keep the active lease"
    );
}

#[test]
fn run_and_record_recovery_required_outcome_keeps_the_lease() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::completed(ReviewResultOutcome::RecoveryRequired, None);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("an uncertain outcome is still recorded");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(
        repository.active_lease.is_some(),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn run_and_record_post_consent_preflight_rejection_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::preflight_rejected();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a post-consent preflight rejection is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn run_and_record_executor_failure_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::failing();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a genuine executor failure is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn panic_containment_recovers_from_a_panicking_executor_records_history_and_keeps_the_lease() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a contained executor panic still records RecoveryRequired");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(
        repository.active_lease.is_some(),
        "RecoveryRequired must keep the active lease even after a contained panic"
    );
    let (_, _, record) = repository.last_saved.expect("a transition was recorded");
    assert_eq!(record.from_state(), Some(TaskState::Reviewing));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    let stored = repository
        .review_results
        .get(&task_id)
        .expect("a review result row was recorded for the contained panic");
    assert_eq!(stored.outcome, ReviewResultOutcome::RecoveryRequired);
    assert_eq!(stored.review_text, None);
}

#[test]
fn panic_containment_never_lets_the_panic_payload_reach_the_recorded_result() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            4,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a contained executor panic still records a result");

    let rendered = format!("{view:?} {:?}", repository.review_results.get(&task_id));
    assert!(
        !rendered.contains(PANIC_SENTINEL),
        "the panic payload must never surface in the recorded TaskView or review result"
    );
}

#[test]
fn panic_containment_does_not_report_success_when_the_recovery_write_is_itself_rejected() {
    let (mut repository, task_id) = setup_reviewing_only(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);
    let stale_expected_version = 99;

    let error = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            stale_expected_version,
            WORKTREE_PATH,
            &brief,
            "diff --git a/f b/f\n",
            20,
            &mut executor,
            &cancellation,
        )
        .expect_err("a rejected recovery write must never be reported as success");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::Reviewing,
        "the task must be left exactly as it was, not silently advanced to any recovery state"
    );
    assert!(
        repository.active_lease.is_some(),
        "the lease must remain untouched when the recovery write is rejected"
    );
    assert!(
        repository.last_saved.is_none(),
        "no transition may be recorded when the recovery write is rejected"
    );
}

#[test]
fn begin_then_run_and_record_connects_consent_diff_and_result_end_to_end() {
    let (mut repository, task_id) = setup_reviewing(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut git = GitFake {
        verified: true,
        calls: 0,
    };
    let mut filesystem = FilesystemFake {
        verify_ok: true,
        calls: 0,
    };
    let mut diff = diff_of("diff --git a/f b/f\n+added a line\n");

    let inputs: ReviewExecutionInputs = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut git,
        &mut filesystem,
        &mut diff,
    )
    .begin(BeginReviewExecutionRequest::new(task_id, 3))
    .expect("begin succeeds");
    assert_eq!(inputs.task.state, TaskState::Reviewing);
    assert_eq!(inputs.task.version, 3);

    let consent = repository
        .get_provider_consent(task_id, ProviderKind::Claude, WorkKind::Review, 3)
        .expect("consent lookup")
        .expect("consent recorded exactly once by begin");
    assert_eq!(consent.approved_task_version, 3);

    let mut executor =
        ScriptedExecutor::completed(ReviewResultOutcome::Completed, Some("looks good"));
    let cancellation = AtomicCancellationSignal::new();

    let view = ReviewExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            inputs.task.version,
            &inputs.worktree_path,
            &inputs.brief,
            &inputs.diff_text,
            40,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::AwaitingUserDiffApproval);
    assert_eq!(executor.observed[0].0, Path::new(&inputs.worktree_path));
    assert_eq!(
        executor.observed[0].4,
        "diff --git a/f b/f\n+added a line\n"
    );
}
