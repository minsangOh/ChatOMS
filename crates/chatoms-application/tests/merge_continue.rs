mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    error::ApplicationErrorCode,
    merge_continue::{BeginMergeContinueRequest, MergeContinueRecorder, MergeContinueStarter},
};
use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, TaskId, TaskState, TaskStateTransition,
    TaskStateTransitionId, TaskStateTransitionSnapshot, ValidationCommandKind,
    ValidationExecutionScope,
};
use chatoms_ports::{
    diff::DiffContentHash,
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::RepositoryKind,
    manual_merge_resolution::{
        ManualMergeResolutionCandidatePort, ManualMergeResolutionCandidateRequest,
        ManualResolutionCandidate, ManualResolutionCandidateOutcome, ManualResolutionDigest,
    },
    merge_continue::{MergeContinueOutcome, MergeContinuePort, MergeContinueRequest},
    repository::{
        DiffApprovalRecord, GitIsolationStatus, ManualMergeResolutionConfirmationRecord,
        ProjectFilesystemIdentityRecord, ProjectRecord, TaskGitIsolation,
        ValidationCommandApprovalRecord,
    },
};

use support::{FakeRepository, FakeTime, restored_task};

const ROOT_PATH: &str = "C:/projects/root";
const COMMON_PATH: &str = "C:/projects/root/.git";
const WORKTREE_PATH: &str = "C:/managed/task";

struct FilesystemFake;

impl FilesystemIdentityPort for FilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        let normalized = path.to_string_lossy().replace('\\', "/");
        match normalized.as_str() {
            ROOT_PATH => Ok(identity(ROOT_PATH, "00000000000000000000000000000001")),
            COMMON_PATH => Ok(identity(COMMON_PATH, "00000000000000000000000000000002")),
            WORKTREE_PATH => Ok(identity(WORKTREE_PATH, "00000000000000000000000000000003")),
            _ => Err(PortFailure::new(FailureCategory::NotFound)),
        }
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        _path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

struct CandidateFake {
    outcome: ManualResolutionCandidateOutcome,
    calls: usize,
}

impl ManualMergeResolutionCandidatePort for CandidateFake {
    fn resolution_candidate(
        &mut self,
        _request: &ManualMergeResolutionCandidateRequest,
    ) -> ManualResolutionCandidateOutcome {
        self.calls += 1;
        self.outcome.clone()
    }
}

struct ScriptedExecutor {
    outcome: MergeContinueOutcome,
    calls: usize,
    panics: bool,
}

impl MergeContinuePort for ScriptedExecutor {
    fn continue_merge(&mut self, _request: &MergeContinueRequest) -> MergeContinueOutcome {
        self.calls += 1;
        assert!(!self.panics, "merge-continue executor panic");
        self.outcome
    }
}

fn identity(path: &str, file_id_hex: &str) -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: PathBuf::from(path),
        volume_serial_hex: "00000000000000000000000000000000".to_owned(),
        file_id_hex: file_id_hex.to_owned(),
    }
}

fn digest(byte: u8) -> ManualResolutionDigest {
    ManualResolutionDigest::from_digest_bytes([byte; 32])
}

fn ready_candidate() -> ManualResolutionCandidateOutcome {
    ManualResolutionCandidateOutcome::Ready(ManualResolutionCandidate {
        base_commit: "a".repeat(40),
        task_commit: "b".repeat(40),
        merge_head_commit: "b".repeat(40),
        resolution_digest: digest(9),
    })
}

fn transition(
    task_id: TaskId,
    sequence: u64,
    from_state: TaskState,
    to_state: TaskState,
    task_version: u64,
) -> TaskStateTransition {
    TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id,
        sequence,
        from_state: Some(from_state),
        to_state,
        task_version,
        actor_kind: "test.actor".parse::<ActorKind>().expect("actor"),
        reason_code: "test.reason".parse::<ReasonCode>().expect("reason"),
        occurred_at_ms: 10 + sequence as i64,
    })
    .expect("transition snapshot")
}

fn project_root_approval(
    task_id: TaskId,
    project_id: ProjectId,
    version: u64,
    kind: ValidationCommandKind,
) -> ValidationCommandApprovalRecord {
    ValidationCommandApprovalRecord {
        task_id,
        approved_task_version: version,
        execution_scope: ValidationExecutionScope::ProjectRoot,
        kind,
        executable: "cargo".to_owned(),
        arguments: vec!["test".to_owned(), "--workspace".to_owned()],
        approved_executable_path: "C:/tools/cargo/bin/cargo.exe".to_owned(),
        executable_volume_serial_hex: "0000000000000001".to_owned(),
        executable_file_id_hex: "00000000000000000000000000000001".to_owned(),
        tool_directory_path: "C:/tools/cargo/bin".to_owned(),
        tool_directory_volume_serial_hex: "0000000000000001".to_owned(),
        tool_directory_file_id_hex: "00000000000000000000000000000002".to_owned(),
        approved_cargo_home_path: None,
        cargo_home_volume_serial_hex: None,
        cargo_home_file_id_hex: None,
        approved_rustup_home_path: None,
        rustup_home_volume_serial_hex: None,
        rustup_home_file_id_hex: None,
        target_project_id: Some(project_id),
        target_project_identity_revision: Some(1),
        target_root_volume_serial_hex: Some("0000000000000001".to_owned()),
        target_root_file_id_hex: Some("00000000000000000000000000000001".to_owned()),
        approved_at_ms: 21,
    }
}

fn configured_repository() -> (FakeRepository, TaskId) {
    let (task, mut history) = restored_task(TaskState::MergeConflict, 3, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    history.extend([
        transition(
            task_id,
            2,
            TaskState::Created,
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            TaskState::AwaitingUserDiffApproval,
            TaskState::Merging,
            2,
        ),
        transition(task_id, 4, TaskState::Merging, TaskState::MergeConflict, 3),
    ]);
    let mut repository = FakeRepository::default();
    repository.project_records.insert(
        project_id,
        ProjectRecord {
            id: project_id,
            name: "Example".to_owned(),
            root_path: ROOT_PATH.to_owned(),
            canonical_path_key: ROOT_PATH.to_ascii_lowercase(),
            display_path: ROOT_PATH.to_owned(),
            created_at_ms: 1,
            updated_at_ms: 1,
        },
    );
    repository.project_identities.insert(
        project_id,
        ProjectFilesystemIdentityRecord {
            project_id,
            root_volume_serial_hex: "00000000000000000000000000000000".to_owned(),
            root_file_id_hex: "00000000000000000000000000000001".to_owned(),
            repository_kind: RepositoryKind::Git,
            git_common_volume_serial_hex: Some("00000000000000000000000000000000".to_owned()),
            git_common_file_id_hex: Some("00000000000000000000000000000002".to_owned()),
            confirmed: true,
            revision: 1,
            verified_at_ms: 2,
        },
    );
    repository.isolations.insert(
        task_id,
        TaskGitIsolation {
            task_id,
            project_id,
            status: GitIsolationStatus::WorktreeReady,
            operation_id: None,
            expected_task_version: 3,
            base_branch: Some("main".to_owned()),
            base_commit: Some("a".repeat(40)),
            worktree_path: Some(WORKTREE_PATH.to_owned()),
            branch_created_by_app: true,
            worktree_created_by_app: true,
            created_at_ms: 1,
            updated_at_ms: 2,
        },
    );
    let hash = DiffContentHash::from_digest_bytes([7; 32]);
    repository.diff_approvals.insert(
        (task_id, 1, hash),
        DiffApprovalRecord {
            task_id,
            approved_task_version: 1,
            diff_content_hash: hash,
            approved_at_ms: 2,
        },
    );
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        repository.project_root_validation_approvals.insert(
            (task_id, 1, kind),
            project_root_approval(task_id, project_id, 1, kind),
        );
    }
    repository.seed_task(task, history);
    (repository, task_id)
}

fn with_confirmation(mut repository: FakeRepository, task_id: TaskId) -> FakeRepository {
    repository.manual_merge_resolution_confirmations.insert(
        (task_id, 3, digest(9)),
        ManualMergeResolutionConfirmationRecord {
            task_id,
            merge_conflict_task_version: 3,
            source_approval_task_version: 1,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            resolution_digest: digest(9),
            confirmed_at_ms: 25,
        },
    );
    repository
}

fn setup_merging(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Merging, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    (repository, task_id)
}

fn minimal_request(task_id: TaskId, project_id: ProjectId) -> MergeContinueRequest {
    MergeContinueRequest {
        original_checkout: identity(ROOT_PATH, "00000000000000000000000000000001"),
        original_common_dir: identity(COMMON_PATH, "00000000000000000000000000000002"),
        task_worktree: identity(WORKTREE_PATH, "00000000000000000000000000000003"),
        project_id,
        task_id,
        merge_conflict_task_version: 3,
        source_approval_task_version: 1,
        base_branch: "main".to_owned(),
        task_branch: format!("ai-task/{task_id}"),
        base_commit: "a".repeat(40),
        task_commit: "b".repeat(40),
        merge_head_commit: "b".repeat(40),
        confirmed_resolution_digest: digest(9),
    }
}

#[test]
fn begin_requires_an_exact_confirmation_before_transitioning() {
    let (mut repository, task_id) = configured_repository();
    let before = repository.tasks.get(&task_id).cloned();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut candidate = CandidateFake {
        outcome: ready_candidate(),
        calls: 0,
    };

    let error =
        MergeContinueStarter::new(&mut repository, &mut time, &mut filesystem, &mut candidate)
            .begin(BeginMergeContinueRequest::new(task_id, 3))
            .expect_err("missing confirmation rejects merge-continue start");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(repository.tasks.get(&task_id), before.as_ref());
    assert!(repository.last_saved.is_none());
}

#[test]
fn begin_transitions_to_merging_once_confirmed_and_recorder_advances_to_post_merge_testing() {
    let (repository, task_id) = configured_repository();
    let project_id = repository.tasks[&task_id].project_id();
    let mut repository = with_confirmation(repository, task_id);
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut candidate = CandidateFake {
        outcome: ready_candidate(),
        calls: 0,
    };

    let inputs =
        MergeContinueStarter::new(&mut repository, &mut time, &mut filesystem, &mut candidate)
            .begin(BeginMergeContinueRequest::new(task_id, 3))
            .expect("all preconditions hold");
    assert_eq!(inputs.task.state, TaskState::Merging);
    assert_eq!(inputs.task.version, 4);
    assert_eq!(inputs.request.confirmed_resolution_digest, digest(9));
    assert_eq!(inputs.request.task_commit, "b".repeat(40));

    let mut executor = ScriptedExecutor {
        outcome: MergeContinueOutcome::Continued,
        calls: 0,
        panics: false,
    };
    let view = MergeContinueRecorder::new(&mut repository, &mut time)
        .run_and_record(task_id, 4, &inputs.request, &mut executor)
        .expect("successful continue records post-merge testing");
    assert_eq!(view.state, TaskState::PostMergeTesting);
    assert_eq!(executor.calls, 1);
    let _ = project_id;
}

#[test]
fn duplicate_start_cannot_execute_twice() {
    let (repository, task_id) = configured_repository();
    let mut repository = with_confirmation(repository, task_id);
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut candidate = CandidateFake {
        outcome: ready_candidate(),
        calls: 0,
    };
    MergeContinueStarter::new(&mut repository, &mut time, &mut filesystem, &mut candidate)
        .begin(BeginMergeContinueRequest::new(task_id, 3))
        .expect("first start succeeds");

    let error =
        MergeContinueStarter::new(&mut repository, &mut time, &mut filesystem, &mut candidate)
            .begin(BeginMergeContinueRequest::new(task_id, 3))
            .expect_err("second start against the same stale version must fail closed");
    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn stale_pending_rejected_and_uncertain_outcomes_all_record_expected_states() {
    for (outcome, expected) in [
        (MergeContinueOutcome::Continued, TaskState::PostMergeTesting),
        (
            MergeContinueOutcome::ConfirmationStale,
            TaskState::MergeConflict,
        ),
        (
            MergeContinueOutcome::ConfirmedMergePending,
            TaskState::MergeConflict,
        ),
        (
            MergeContinueOutcome::PreWriteRejected,
            TaskState::RecoveryRequired,
        ),
        (
            MergeContinueOutcome::PostWriteUncertain,
            TaskState::RecoveryRequired,
        ),
    ] {
        let (mut repository, task_id) = setup_merging(4);
        let project_id = repository.tasks[&task_id].project_id();
        let mut time = FakeTime::at(30);
        let mut executor = ScriptedExecutor {
            outcome,
            calls: 0,
            panics: false,
        };
        let view = MergeContinueRecorder::new(&mut repository, &mut time)
            .run_and_record(
                task_id,
                4,
                &minimal_request(task_id, project_id),
                &mut executor,
            )
            .expect("outcome is recorded");
        assert_eq!(view.state, expected, "{outcome:?}");
    }
}

#[test]
fn executor_panic_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_merging(4);
    let project_id = repository.tasks[&task_id].project_id();
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeContinueOutcome::Continued,
        calls: 0,
        panics: true,
    };
    let view = MergeContinueRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            4,
            &minimal_request(task_id, project_id),
            &mut executor,
        )
        .expect("panic falls back to recovery required");
    assert_eq!(view.state, TaskState::RecoveryRequired);
}

#[test]
fn result_persistence_failure_does_not_report_success() {
    let (mut repository, task_id) = setup_merging(4);
    let project_id = repository.tasks[&task_id].project_id();
    repository.fail_on = Some((
        "save_transition",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeContinueOutcome::Continued,
        calls: 0,
        panics: false,
    };
    let view = MergeContinueRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            &minimal_request(task_id, project_id),
            &mut executor,
        )
        .expect("failed primary persistence falls back to recovery");
    assert_eq!(view.state, TaskState::RecoveryRequired);
}
