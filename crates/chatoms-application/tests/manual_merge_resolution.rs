mod support;

use std::path::{Path, PathBuf};

use chatoms_application::manual_merge_resolution::{
    ConfirmManualMergeResolutionRequest, ManualMergeResolutionConfirmationService,
};
use chatoms_domain::{
    ActorKind, ReasonCode, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot, ValidationCommandKind, ValidationExecutionScope,
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
    repository::{
        DiffApprovalRecord, GitIsolationStatus, ProjectFilesystemIdentityRecord, ProjectRecord,
        TaskGitIsolation, ValidationCommandApprovalRecord,
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
    project_id: chatoms_domain::ProjectId,
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

#[test]
fn confirm_records_an_immutable_confirmation_for_the_live_candidate_digest() {
    let (mut repository, task_id) = configured_repository();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut candidate = CandidateFake {
        outcome: ready_candidate(),
        calls: 0,
    };

    let view = ManualMergeResolutionConfirmationService::new(
        &mut repository,
        &mut time,
        &mut filesystem,
        &mut candidate,
    )
    .confirm(ConfirmManualMergeResolutionRequest::new(task_id, 3))
    .expect("preconditions hold");

    assert_eq!(view.task_id, task_id);
    assert_eq!(view.merge_conflict_task_version, 3);
    assert_eq!(view.source_approval_task_version, 1);
    assert_eq!(view.resolution_digest, digest(9));
    assert_eq!(candidate.calls, 1);
    assert_eq!(
        repository
            .manual_merge_resolution_confirmations
            .get(&(task_id, 3, digest(9)))
            .map(|record| record.confirmed_at_ms),
        Some(30)
    );
    assert!(
        repository
            .calls
            .iter()
            .all(|call| !call.starts_with("save_transition") && *call != "terminate_task")
    );
}

#[test]
fn confirm_is_idempotent_for_the_same_live_digest() {
    let (mut repository, task_id) = configured_repository();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut candidate = CandidateFake {
        outcome: ready_candidate(),
        calls: 0,
    };
    ManualMergeResolutionConfirmationService::new(
        &mut repository,
        &mut time,
        &mut filesystem,
        &mut candidate,
    )
    .confirm(ConfirmManualMergeResolutionRequest::new(task_id, 3))
    .expect("first confirmation");

    let mut time = FakeTime::at(45);
    let second = ManualMergeResolutionConfirmationService::new(
        &mut repository,
        &mut time,
        &mut filesystem,
        &mut candidate,
    )
    .confirm(ConfirmManualMergeResolutionRequest::new(task_id, 3))
    .expect("second confirmation reuses the row");

    assert_eq!(
        second.confirmed_at_ms, 30,
        "the original row is kept, not replaced"
    );
    assert_eq!(repository.manual_merge_resolution_confirmations.len(), 1);
}

#[test]
fn wrong_state_version_lease_history_approval_or_identity_fails_closed_without_writing() {
    for case in [
        "state",
        "version",
        "lease",
        "history",
        "diff_approval",
        "project_root_approval",
        "identity",
    ] {
        let (mut repository, task_id) = configured_repository();
        match case {
            "state" => {
                let task = repository.tasks.get_mut(&task_id).expect("task");
                *task = chatoms_domain::Task::restore(chatoms_domain::TaskSnapshot {
                    id: task.id(),
                    project_id: task.project_id(),
                    state: TaskState::Merging,
                    version: task.version(),
                    task_branch_identity: task.task_branch_identity().clone(),
                    resume_target_state: None,
                    created_at_ms: task.created_at_ms(),
                    updated_at_ms: task.updated_at_ms(),
                    terminal_at_ms: None,
                })
                .expect("restored task");
            }
            "version" => {
                repository
                    .isolations
                    .get_mut(&task_id)
                    .expect("isolation")
                    .expected_task_version = 99;
            }
            "lease" => repository.active_lease = None,
            "history" => {
                repository.transitions.insert(task_id, Vec::new());
            }
            "diff_approval" => repository.diff_approvals.clear(),
            "project_root_approval" => repository.project_root_validation_approvals.clear(),
            "identity" => {
                repository
                    .project_identities
                    .get_mut(&repository.tasks[&task_id].project_id())
                    .expect("project identity")
                    .root_file_id_hex = "00000000000000000000000000000009".to_owned();
            }
            _ => unreachable!("all mismatch cases are listed"),
        }
        let mut time = FakeTime::at(30);
        let mut filesystem = FilesystemFake;
        let mut candidate = CandidateFake {
            outcome: ready_candidate(),
            calls: 0,
        };

        let error = ManualMergeResolutionConfirmationService::new(
            &mut repository,
            &mut time,
            &mut filesystem,
            &mut candidate,
        )
        .confirm(ConfirmManualMergeResolutionRequest::new(task_id, 3))
        .expect_err(&format!("case {case} must fail closed"));
        let _ = error;

        assert_eq!(candidate.calls, 0, "{case} must not read the candidate");
        assert!(
            repository.manual_merge_resolution_confirmations.is_empty(),
            "{case} must not create a confirmation row"
        );
    }
}

#[test]
fn unresolved_inconsistent_or_unavailable_candidate_fails_closed_without_writing() {
    for outcome in [
        ManualResolutionCandidateOutcome::Unresolved,
        ManualResolutionCandidateOutcome::Inconsistent,
        ManualResolutionCandidateOutcome::Unavailable,
    ] {
        let (mut repository, task_id) = configured_repository();
        let mut time = FakeTime::at(30);
        let mut filesystem = FilesystemFake;
        let mut candidate = CandidateFake {
            outcome: outcome.clone(),
            calls: 0,
        };

        let error = ManualMergeResolutionConfirmationService::new(
            &mut repository,
            &mut time,
            &mut filesystem,
            &mut candidate,
        )
        .confirm(ConfirmManualMergeResolutionRequest::new(task_id, 3))
        .expect_err("non-Ready candidate must fail closed");

        assert_eq!(candidate.calls, 1);
        assert!(repository.manual_merge_resolution_confirmations.is_empty());
        let _ = error;
    }
}
