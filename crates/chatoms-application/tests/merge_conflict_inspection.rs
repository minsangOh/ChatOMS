mod support;

use std::path::{Path, PathBuf};

use chatoms_application::merge_conflict_inspection::MergeConflictInspectionService;
use chatoms_domain::{
    ActorKind, ReasonCode, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot,
};
use chatoms_ports::{
    diff::DiffContentHash,
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::RepositoryKind,
    merge_conflict_inspection::{
        MergeConflictInspectionOutcome, MergeConflictInspectionPort,
        MergeConflictInspectionRequest, MergeConflictInspectionResult,
    },
    repository::{
        DiffApprovalRecord, GitIsolationStatus, ProjectFilesystemIdentityRecord, ProjectRecord,
        TaskGitIsolation,
    },
};

use support::{FakeRepository, restored_task};

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

struct InspectionFake {
    calls: usize,
}

impl MergeConflictInspectionPort for InspectionFake {
    fn inspect_merge_conflicts(
        &mut self,
        _request: &MergeConflictInspectionRequest,
    ) -> MergeConflictInspectionResult {
        self.calls += 1;
        MergeConflictInspectionResult {
            outcome: MergeConflictInspectionOutcome::ConfirmedUnresolved,
            counts: Default::default(),
        }
    }
}

fn identity(path: &str, file_id_hex: &str) -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: PathBuf::from(path),
        volume_serial_hex: "00000000000000000000000000000000".to_owned(),
        file_id_hex: file_id_hex.to_owned(),
    }
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
            expected_task_version: 1,
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
    repository.seed_task(task, history);
    (repository, task_id)
}

#[test]
fn reinspection_is_read_only_and_preserves_the_active_lease() {
    let (mut repository, task_id) = configured_repository();
    let lease_before = repository.active_lease;
    let mut filesystem = FilesystemFake;
    let mut git = InspectionFake { calls: 0 };

    let result = MergeConflictInspectionService::new(&mut repository, &mut filesystem, &mut git)
        .inspect(task_id)
        .expect("inspection should complete")
        .expect("merge-conflict task should be inspected");

    assert_eq!(
        result.outcome,
        MergeConflictInspectionOutcome::ConfirmedUnresolved
    );
    assert_eq!(git.calls, 1);
    assert_eq!(repository.active_lease, lease_before);
    assert!(
        repository
            .calls
            .iter()
            .all(|call| !call.starts_with("save_"))
    );
}

#[test]
fn stale_version_lease_history_approval_and_identity_mismatches_fail_closed() {
    for case in ["stale_chain", "lease", "history", "approval", "identity"] {
        let (mut repository, task_id) = configured_repository();
        match case {
            // `inspect` takes no expected version: the binding it enforces
            // is that the merge chain ends at the task's *current* version.
            // Advancing the task past the end of its recorded history must
            // therefore resolve no provenance at all.
            // `TaskGitIsolation.expected_task_version` is deliberately not a
            // case here — it is the isolation lifecycle's own concurrency
            // value, frozen at `WorktreeReady` and always behind a
            // `MergeConflict` task (see
            // `tests/merge_lifecycle_reachability.rs`).
            "stale_chain" => {
                let task = repository.tasks.get_mut(&task_id).expect("task");
                *task = chatoms_domain::Task::restore(chatoms_domain::TaskSnapshot {
                    id: task.id(),
                    project_id: task.project_id(),
                    state: task.state(),
                    version: task.version() + 1,
                    task_branch_identity: task.task_branch_identity().clone(),
                    resume_target_state: None,
                    created_at_ms: task.created_at_ms(),
                    updated_at_ms: task.updated_at_ms(),
                    terminal_at_ms: None,
                })
                .expect("restored task");
            }
            "lease" => repository.active_lease = None,
            "history" => {
                repository.transitions.insert(task_id, Vec::new());
            }
            "approval" => repository.diff_approvals.clear(),
            "identity" => {
                repository
                    .project_identities
                    .get_mut(&repository.tasks[&task_id].project_id())
                    .expect("project identity")
                    .root_file_id_hex = "00000000000000000000000000000009".to_owned()
            }
            _ => unreachable!("all mismatch cases are listed"),
        };
        let mut filesystem = FilesystemFake;
        let mut git = InspectionFake { calls: 0 };
        let result =
            MergeConflictInspectionService::new(&mut repository, &mut filesystem, &mut git)
                .inspect(task_id)
                .expect("inspection should return a typed result")
                .expect("merge-conflict task should produce a result");

        assert_eq!(
            result.outcome,
            MergeConflictInspectionOutcome::Inconsistent,
            "{case}"
        );
        assert_eq!(git.calls, 0, "{case} must not inspect Git");
        assert!(
            repository
                .calls
                .iter()
                .all(|call| !call.starts_with("save_"))
        );
    }
}

#[test]
fn non_merge_conflict_states_return_no_inspection() {
    let (task, history) = support::restored_task(TaskState::Completed, 4, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    let mut filesystem = FilesystemFake;
    let mut git = InspectionFake { calls: 0 };

    let result = MergeConflictInspectionService::new(&mut repository, &mut filesystem, &mut git)
        .inspect(task_id)
        .expect("non-conflict state should be safe");

    assert_eq!(result, None);
    assert_eq!(git.calls, 0);
}
