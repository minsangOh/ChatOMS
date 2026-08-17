mod support;

use std::path::Path;

use chatoms_application::{
    error::ApplicationErrorCode,
    merge_execution::{BeginMergeExecutionRequest, MergeExecutionRecorder, MergeExecutionStarter},
};
use chatoms_domain::{
    ProjectId, TaskId, TaskState, ValidationCommandKind, ValidationExecutionScope,
};
use chatoms_ports::{
    diff::DiffContentHash,
    error::PortFailure,
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    merge_execution::{MergeExecutionOutcome, MergeExecutionPort, MergeExecutionRequest},
    repository::{
        DiffApprovalRecord, GitIsolationStatus, ProjectRecord, TaskGitIsolation,
        ValidationCommandApprovalRecord,
    },
};

use support::{FakeRepository, FakeTime, restored_task};

const ROOT_PATH: &str = "C:/projects/example";
const WORKTREE_PATH: &str = "C:/managed/task";

struct FilesystemFake {
    inspected: Vec<String>,
    verify_calls: usize,
}

impl FilesystemIdentityPort for FilesystemFake {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.inspected.push(path.display().to_string());
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000001".to_owned(),
            file_id_hex: format!("{:032x}", self.inspected.len()),
        })
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        self.verify_calls += 1;
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        _path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        unreachable!("merge application orchestration does not acquire guards")
    }
}

struct ScriptedExecutor {
    outcome: MergeExecutionOutcome,
    calls: usize,
    panics: bool,
}

impl MergeExecutionPort for ScriptedExecutor {
    fn commit_and_merge(&mut self, _request: &MergeExecutionRequest) -> MergeExecutionOutcome {
        self.calls += 1;
        assert!(!self.panics, "merge executor panic");
        self.outcome
    }
}

fn hash() -> DiffContentHash {
    DiffContentHash::from_digest_bytes([7; 32])
}

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

fn isolation(task_id: TaskId, project_id: ProjectId, version: u64) -> TaskGitIsolation {
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

fn setup_awaiting(version: u64, approval: bool) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::AwaitingUserDiffApproval, version, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    repository
        .project_records
        .insert(project_id, project_record(project_id));
    repository
        .isolations
        .insert(task_id, isolation(task_id, project_id, version));
    if approval {
        repository.diff_approvals.insert(
            (task_id, version, hash()),
            DiffApprovalRecord {
                task_id,
                approved_task_version: version,
                diff_content_hash: hash(),
                approved_at_ms: 21,
            },
        );
    }
    repository.seed_task(task, history);
    (repository, task_id)
}

fn setup_merging(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Merging, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    (repository, task_id)
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

fn task_worktree_approval(
    task_id: TaskId,
    project_id: ProjectId,
    version: u64,
    kind: ValidationCommandKind,
) -> ValidationCommandApprovalRecord {
    let mut approval = project_root_approval(task_id, project_id, version, kind);
    approval.execution_scope = ValidationExecutionScope::TaskWorktree;
    approval.target_project_id = None;
    approval.target_project_identity_revision = None;
    approval.target_root_volume_serial_hex = None;
    approval.target_root_file_id_hex = None;
    approval
}

#[test]
fn begin_requires_exact_approval_before_transition_or_identity_inspection() {
    let (mut repository, task_id) = setup_awaiting(3, false);
    let before = repository.tasks.get(&task_id).cloned();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake {
        inspected: Vec::new(),
        verify_calls: 0,
    };

    let error = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(task_id, 3, hash()))
        .expect_err("missing approval rejects merge start");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(repository.tasks.get(&task_id), before.as_ref());
    assert!(repository.last_saved.is_none());
    assert!(filesystem.inspected.is_empty());
    assert_eq!(filesystem.verify_calls, 0);
}

#[test]
fn approved_merge_starts_then_records_post_merge_testing() {
    let (mut repository, task_id) = setup_awaiting(3, true);
    let project_id = repository
        .tasks
        .get(&task_id)
        .expect("task exists")
        .project_id();
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        repository.project_root_validation_approvals.insert(
            (task_id, 3, kind),
            project_root_approval(task_id, project_id, 3, kind),
        );
    }
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake {
        inspected: Vec::new(),
        verify_calls: 0,
    };
    let inputs = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(task_id, 3, hash()))
        .expect("all start preconditions hold");
    assert_eq!(inputs.task.state, TaskState::Merging);
    assert_eq!(inputs.task.version, 4);
    assert_eq!(inputs.request.approved_diff_content_hash, hash());
    assert_eq!(filesystem.inspected.len(), 3);
    assert_eq!(filesystem.verify_calls, 2);

    let mut executor = ScriptedExecutor {
        outcome: MergeExecutionOutcome::Merged,
        calls: 0,
        panics: false,
    };
    let view = MergeExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(task_id, 4, &inputs.request, &mut executor)
        .expect("successful merge records post-merge testing");
    assert_eq!(view.state, TaskState::PostMergeTesting);
    assert_eq!(executor.calls, 1);
    assert_eq!(
        repository.active_lease.map(|lease| lease.task_id),
        Some(task_id)
    );
}

#[test]
fn begin_rejects_missing_project_root_test_or_build_before_transition_or_identity_inspection() {
    let (mut repository, task_id) = setup_awaiting(3, true);
    let task = repository.tasks.get(&task_id).expect("task exists");
    let project_id = task.project_id();
    repository.project_root_validation_approvals.insert(
        (task_id, 3, ValidationCommandKind::Test),
        project_root_approval(task_id, project_id, 3, ValidationCommandKind::Test),
    );
    let before = repository.tasks.get(&task_id).cloned();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake {
        inspected: Vec::new(),
        verify_calls: 0,
    };

    let error = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(task_id, 3, hash()))
        .expect_err("missing ProjectRoot Build approval rejects merge start");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(repository.tasks.get(&task_id), before.as_ref());
    assert!(repository.last_saved.is_none());
    assert!(filesystem.inspected.is_empty());
    assert_eq!(filesystem.verify_calls, 0);
}

#[test]
fn begin_does_not_substitute_task_worktree_approvals_for_project_root_approvals() {
    let (mut repository, task_id) = setup_awaiting(3, true);
    let project_id = repository
        .tasks
        .get(&task_id)
        .expect("task exists")
        .project_id();
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        repository.validation_command_approvals.insert(
            (task_id, 3, kind),
            task_worktree_approval(task_id, project_id, 3, kind),
        );
    }
    let before = repository.tasks.get(&task_id).cloned();
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake {
        inspected: Vec::new(),
        verify_calls: 0,
    };

    let error = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(task_id, 3, hash()))
        .expect_err("TaskWorktree approvals must not authorize ProjectRoot merge validation");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(repository.tasks.get(&task_id), before.as_ref());
    assert!(repository.last_saved.is_none());
    assert!(filesystem.inspected.is_empty());
    assert_eq!(filesystem.verify_calls, 0);
}

#[test]
fn confirmed_conflict_records_merge_conflict_without_success_transition() {
    let (mut repository, task_id) = setup_merging(4);
    let mut time = FakeTime::at(30);
    let request = MergeExecutionRequest {
        original_checkout: identity(ROOT_PATH, 1),
        original_common_dir: identity("C:/projects/example/.git", 2),
        task_worktree: identity(WORKTREE_PATH, 3),
        task_branch: format!("ai-task/{task_id}"),
        base_branch: "main".to_owned(),
        base_commit: "a".repeat(40),
        approved_diff_content_hash: hash(),
    };
    let mut executor = ScriptedExecutor {
        outcome: MergeExecutionOutcome::ConfirmedMergeConflict,
        calls: 0,
        panics: false,
    };

    let view = MergeExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(task_id, 4, &request, &mut executor)
        .expect("confirmed conflict is recorded");

    assert_eq!(view.state, TaskState::MergeConflict);
    assert_ne!(view.state, TaskState::PostMergeTesting);
    assert_eq!(executor.calls, 1);
}

#[test]
fn uncertain_outcome_and_executor_panic_both_record_recovery_required() {
    let (mut repository, task_id) = setup_merging(4);
    let mut time = FakeTime::at(30);
    let request = minimal_request(task_id);
    let mut executor = ScriptedExecutor {
        outcome: MergeExecutionOutcome::StageWriteUncertain,
        calls: 0,
        panics: false,
    };
    let view = MergeExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(task_id, 4, &request, &mut executor)
        .expect("uncertain outcome is recorded fail-closed");
    assert_eq!(view.state, TaskState::RecoveryRequired);

    let (mut repository, task_id) = setup_merging(4);
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeExecutionOutcome::Merged,
        calls: 0,
        panics: true,
    };
    let view = MergeExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(task_id, 4, &minimal_request(task_id), &mut executor)
        .expect("panic falls back to recovery required");
    assert_eq!(view.state, TaskState::RecoveryRequired);
}

#[test]
fn stale_start_and_result_persistence_failure_do_not_report_success() {
    let (mut repository, task_id) = setup_awaiting(3, true);
    let mut time = FakeTime::at(30);
    let mut filesystem = FilesystemFake {
        inspected: Vec::new(),
        verify_calls: 0,
    };
    let error = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(task_id, 2, hash()))
        .expect_err("stale start rejects before writes");
    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(
        repository.tasks.get(&task_id).map(|task| task.state()),
        Some(TaskState::AwaitingUserDiffApproval)
    );

    let (mut repository, task_id) = setup_merging(4);
    repository.fail_on = Some((
        "save_transition",
        chatoms_ports::repository::RepositoryErrorCode::OperationFailed,
    ));
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor {
        outcome: MergeExecutionOutcome::Merged,
        calls: 0,
        panics: false,
    };
    let view = MergeExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(task_id, 4, &minimal_request(task_id), &mut executor)
        .expect("failed primary persistence falls back to recovery");
    assert_eq!(view.state, TaskState::RecoveryRequired);
}

fn identity(path: &str, id: u8) -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: path.into(),
        volume_serial_hex: "0000000000000001".to_owned(),
        file_id_hex: format!("{id:032x}"),
    }
}

fn minimal_request(task_id: TaskId) -> MergeExecutionRequest {
    MergeExecutionRequest {
        original_checkout: identity(ROOT_PATH, 1),
        original_common_dir: identity("C:/projects/example/.git", 2),
        task_worktree: identity(WORKTREE_PATH, 3),
        task_branch: format!("ai-task/{task_id}"),
        base_branch: "main".to_owned(),
        base_commit: "a".repeat(40),
        approved_diff_content_hash: hash(),
    }
}
