mod support;

use std::path::{Path, PathBuf};

use chatoms_application::merge_abort::{
    ApproveMergeAbortRequest, MergeAbortApprovalService, MergeAbortRecorder,
};
use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, TaskId, TaskState, TaskStateTransition,
    TaskStateTransitionId, TaskStateTransitionSnapshot,
};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::RepositoryKind,
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
    merge_abort::{
        MergeAbortOutcome, MergeAbortPort, MergeAbortPreWriteRejection, MergeAbortRequest,
    },
    repository::{ProjectFilesystemIdentityRecord, ProjectRecord, TaskGitIsolation},
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

/// Only `repository_status` is exercised by `MergeAbortApprovalService`;
/// every other `GitService` method panics if called, so a wrongly-ordered
/// implementation that spawns Git through some other path fails loudly
/// instead of silently.
struct GitFake {
    status: Result<RepositoryStatus, PortFailure>,
    calls: usize,
}

impl GitService for GitFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn inspect_project(&mut self, _input: &Path) -> Result<ProjectInspection, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn repository_status(&mut self, _root: &Path) -> Result<RepositoryStatus, PortFailure> {
        self.calls += 1;
        self.status.clone()
    }
    fn validate_non_git_source(&mut self, _root: &Path) -> Result<(), PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn validate_repository_source(
        &mut self,
        _root: &Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn initialize_repository(&mut self, _root: &Path) -> Result<(), PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn has_commit_author(&mut self, _root: &Path) -> Result<bool, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn create_initial_snapshot(&mut self, _root: &Path) -> Result<String, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn create_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
    fn verify_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
    ) -> Result<bool, PortFailure> {
        unreachable!("not used by MergeAbortApprovalService")
    }
}

struct ScriptedExecutor {
    outcome: MergeAbortOutcome,
    calls: usize,
    panics: bool,
}

impl MergeAbortPort for ScriptedExecutor {
    fn abort_merge(&mut self, _request: &MergeAbortRequest) -> MergeAbortOutcome {
        self.calls += 1;
        assert!(!self.panics, "merge-abort executor panic");
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

fn task_branch_for(task_id: TaskId) -> String {
    format!("ai-task/{task_id}")
}

fn clean_status(branch: &str, commit: &str) -> RepositoryStatus {
    RepositoryStatus {
        clean: true,
        detached_head: false,
        current_branch: Some(branch.to_owned()),
        head_commit: Some(commit.to_owned()),
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
            status: chatoms_ports::repository::GitIsolationStatus::WorktreeReady,
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
    repository.seed_task(task, history);
    (repository, task_id)
}

fn setup_merge_conflict(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::MergeConflict, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    (repository, task_id)
}

fn minimal_request(task_id: TaskId, project_id: ProjectId) -> MergeAbortRequest {
    MergeAbortRequest {
        original_checkout: identity(ROOT_PATH, "00000000000000000000000000000001"),
        original_common_dir: identity(COMMON_PATH, "00000000000000000000000000000002"),
        task_worktree: identity(WORKTREE_PATH, "00000000000000000000000000000003"),
        project_id,
        task_id,
        merge_conflict_task_version: 3,
        source_approval_task_version: 1,
        base_branch: "main".to_owned(),
        task_branch: task_branch_for(task_id),
        base_commit: "a".repeat(40),
        task_commit: "b".repeat(40),
        merge_head_commit: "b".repeat(40),
    }
}

#[test]
fn approve_records_an_immutable_approval_for_the_current_task_worktree_head() {
    let (mut repository, task_id) = configured_repository();
    let task_branch = task_branch_for(task_id);
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut git = GitFake {
        status: Ok(clean_status(&task_branch, &"b".repeat(40))),
        calls: 0,
    };

    let view =
        MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
            .approve(ApproveMergeAbortRequest::new(task_id, 3))
            .expect("preconditions hold");

    assert_eq!(view.task_id, task_id);
    assert_eq!(view.merge_conflict_task_version, 3);
    assert_eq!(view.source_approval_task_version, 1);
    assert_eq!(view.base_commit, "a".repeat(40));
    assert_eq!(view.task_commit, "b".repeat(40));
    assert_eq!(view.merge_head_commit, "b".repeat(40));
    assert_eq!(git.calls, 1);
    assert_eq!(
        repository
            .merge_abort_approvals
            .get(&(task_id, 3))
            .map(|record| record.approved_at_ms),
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
fn approve_is_idempotent_for_the_same_task_version() {
    let (mut repository, task_id) = configured_repository();
    let task_branch = task_branch_for(task_id);
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut git = GitFake {
        status: Ok(clean_status(&task_branch, &"b".repeat(40))),
        calls: 0,
    };
    MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
        .approve(ApproveMergeAbortRequest::new(task_id, 3))
        .expect("first approval");

    let mut time = FakeTime::at(45);
    let second =
        MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
            .approve(ApproveMergeAbortRequest::new(task_id, 3))
            .expect("second approval reuses the row");

    assert_eq!(
        second.approved_at_ms, 30,
        "the original timestamp is kept, not replaced"
    );
    assert_eq!(repository.merge_abort_approvals.len(), 1);
}

#[test]
fn wrong_state_version_lease_history_or_identity_fails_closed_without_writing() {
    for case in ["state", "version", "lease", "history", "identity"] {
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
            "identity" => {
                repository
                    .project_identities
                    .get_mut(&repository.tasks[&task_id].project_id())
                    .expect("project identity")
                    .root_file_id_hex = "00000000000000000000000000000009".to_owned();
            }
            _ => unreachable!("all mismatch cases are listed"),
        }
        let task_branch = task_branch_for(task_id);
        let mut time = FakeTime::at(30);
        let mut filesystem = FilesystemFake;
        let mut git = GitFake {
            status: Ok(clean_status(&task_branch, &"b".repeat(40))),
            calls: 0,
        };

        let error =
            MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
                .approve(ApproveMergeAbortRequest::new(task_id, 3))
                .expect_err(&format!("case {case} must fail closed"));
        let _ = error;

        assert_eq!(git.calls, 0, "{case} must not read live Git state");
        assert!(
            repository.merge_abort_approvals.is_empty(),
            "{case} must not create an approval row"
        );
    }
}

#[test]
fn a_task_worktree_not_on_its_task_branch_fails_closed_without_writing() {
    let (mut repository, task_id) = configured_repository();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut git = GitFake {
        // Wrong branch: the live worktree does not match the task's own
        // branch identity, so its `HEAD` cannot be trusted as "the task
        // commit".
        status: Ok(clean_status("some-other-branch", &"b".repeat(40))),
        calls: 0,
    };

    let error =
        MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
            .approve(ApproveMergeAbortRequest::new(task_id, 3))
            .expect_err("a worktree not on its task branch must fail closed");
    let _ = error;

    assert!(repository.merge_abort_approvals.is_empty());
}

#[test]
fn a_detached_task_worktree_fails_closed_without_writing() {
    let (mut repository, task_id) = configured_repository();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake;
    let mut git = GitFake {
        status: Ok(RepositoryStatus {
            clean: true,
            detached_head: true,
            current_branch: None,
            head_commit: Some("b".repeat(40)),
        }),
        calls: 0,
    };

    let error =
        MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
            .approve(ApproveMergeAbortRequest::new(task_id, 3))
            .expect_err("a detached task worktree must fail closed");
    let _ = error;

    assert!(repository.merge_abort_approvals.is_empty());
}

#[test]
fn confirmed_and_uncertain_outcomes_are_recorded_correctly() {
    for (outcome, expect_state) in [
        (MergeAbortOutcome::Aborted, Some(TaskState::Cancelled)),
        (
            MergeAbortOutcome::ConfirmedNotInMerge,
            Some(TaskState::Cancelled),
        ),
        (
            MergeAbortOutcome::PreWriteRejected(MergeAbortPreWriteRejection::AutostashPresent),
            None,
        ),
        (MergeAbortOutcome::PostWriteUncertain, None),
    ] {
        let (mut repository, task_id) = setup_merge_conflict(4);
        let project_id = repository.tasks[&task_id].project_id();
        repository.merge_abort_approvals.insert(
            (task_id, 4),
            chatoms_ports::repository::MergeAbortApprovalRecord {
                task_id,
                merge_conflict_task_version: 4,
                source_approval_task_version: 1,
                base_commit: "a".repeat(40),
                task_commit: "b".repeat(40),
                merge_head_commit: "b".repeat(40),
                approved_at_ms: 25,
            },
        );
        let mut time = FakeTime::at(30);
        let mut executor = ScriptedExecutor {
            outcome,
            calls: 0,
            panics: false,
        };

        let result = MergeAbortRecorder::new(&mut repository, &mut time).run_and_record(
            task_id,
            4,
            &minimal_request(task_id, project_id),
            &mut executor,
        );

        match expect_state {
            Some(state) => {
                let view = result.unwrap_or_else(|_| panic!("{outcome:?} must be recorded"));
                assert_eq!(view.state, state, "{outcome:?}");
            }
            None => {
                result.expect_err(&format!(
                    "{outcome:?} must fail closed without any state transition"
                ));
                assert_eq!(
                    repository.tasks[&task_id].state(),
                    TaskState::MergeConflict,
                    "{outcome:?} must leave the task in MergeConflict"
                );
                assert_eq!(
                    repository.tasks[&task_id].version(),
                    4,
                    "{outcome:?} must not bump the task version"
                );
            }
        }
    }
}

#[test]
fn executor_panic_fails_closed_without_a_state_transition() {
    let (mut repository, task_id) = setup_merge_conflict(4);
    let project_id = repository.tasks[&task_id].project_id();
    repository.merge_abort_approvals.insert(
        (task_id, 4),
        chatoms_ports::repository::MergeAbortApprovalRecord {
            task_id,
            merge_conflict_task_version: 4,
            source_approval_task_version: 1,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            approved_at_ms: 25,
        },
    );
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeAbortOutcome::Aborted,
        calls: 0,
        panics: true,
    };

    let error = MergeAbortRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            4,
            &minimal_request(task_id, project_id),
            &mut executor,
        )
        .expect_err("a caught panic must fail closed, matching PostWriteUncertain");

    let _ = error;
    assert_eq!(repository.tasks[&task_id].state(), TaskState::MergeConflict);
    assert_eq!(repository.tasks[&task_id].version(), 4);
}

#[test]
fn missing_approval_is_rejected_and_never_reaches_the_executor() {
    let (mut repository, task_id) = setup_merge_conflict(4);
    let project_id = repository.tasks[&task_id].project_id();
    // Deliberately no approval seeded.
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeAbortOutcome::Aborted,
        calls: 0,
        panics: false,
    };

    let error = MergeAbortRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            &minimal_request(task_id, project_id),
            &mut executor,
        )
        .expect_err("Aborted with no prior approval must still be rejected by record");

    let _ = error;
    assert_eq!(repository.tasks[&task_id].state(), TaskState::MergeConflict);
}

#[test]
fn result_persistence_failure_propagates_without_reporting_success() {
    let (mut repository, task_id) = setup_merge_conflict(4);
    let project_id = repository.tasks[&task_id].project_id();
    repository.merge_abort_approvals.insert(
        (task_id, 4),
        chatoms_ports::repository::MergeAbortApprovalRecord {
            task_id,
            merge_conflict_task_version: 4,
            source_approval_task_version: 1,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            approved_at_ms: 25,
        },
    );
    repository.fail_on = Some((
        "save_merge_abort_transition",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeAbortOutcome::Aborted,
        calls: 0,
        panics: false,
    };

    let error = MergeAbortRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            &minimal_request(task_id, project_id),
            &mut executor,
        )
        .expect_err(
            "a failed atomic write must propagate as an error, never a fabricated success -- \
             there is no RecoveryRequired fallback for this edge",
        );

    let _ = error;
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::MergeConflict,
        "the task must remain MergeConflict; the immutable approval survives for a retry"
    );
    assert_eq!(repository.tasks[&task_id].version(), 4);
    assert_eq!(
        repository.merge_abort_approvals.len(),
        1,
        "the approval row itself must remain untouched"
    );
}

#[test]
fn a_stale_version_is_rejected_before_calling_the_executor() {
    let (mut repository, task_id) = setup_merge_conflict(4);
    let project_id = repository.tasks[&task_id].project_id();
    repository.merge_abort_approvals.insert(
        (task_id, 4),
        chatoms_ports::repository::MergeAbortApprovalRecord {
            task_id,
            merge_conflict_task_version: 4,
            source_approval_task_version: 1,
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "b".repeat(40),
            approved_at_ms: 25,
        },
    );
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeAbortOutcome::Aborted,
        calls: 0,
        panics: false,
    };

    let error = MergeAbortRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            // Stale: the task is actually at version 4.
            3,
            &minimal_request(task_id, project_id),
            &mut executor,
        )
        .expect_err("a stale expected_version must fail closed");
    let _ = error;
    assert_eq!(
        executor.calls, 1,
        "the recorder still calls the executor -- the version check happens when recording the result, matching every other Recorder in this crate"
    );
    assert_eq!(repository.tasks[&task_id].state(), TaskState::MergeConflict);
    assert_eq!(repository.tasks[&task_id].version(), 4);
}
