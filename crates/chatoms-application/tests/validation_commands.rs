mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    error::ApplicationErrorCode,
    validation_commands::{
        ApproveProjectRootValidationCommandRequest, ApproveValidationCommandRequest,
        ValidationCommandBindingStatus, ValidationCommandService,
    },
};
use chatoms_domain::{GitOperationId, ProjectId, TaskId, TaskState, ValidationCommandKind};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    repository::{
        GitIsolationStatus, ProjectFilesystemIdentityRecord, ProjectRecord, TaskGitIsolation,
    },
    validation::{ValidationCommandCandidate, ValidationCommandDiscovery},
};

use support::{FakeRepository, FakeTime, restored_task};

struct FakeDiscovery {
    candidates: Vec<ValidationCommandCandidate>,
    observed_paths: Vec<PathBuf>,
    failure: Option<PortFailure>,
}

impl FakeDiscovery {
    fn with_candidates(candidates: Vec<ValidationCommandCandidate>) -> Self {
        Self {
            candidates,
            observed_paths: Vec::new(),
            failure: None,
        }
    }

    fn failing() -> Self {
        Self {
            candidates: Vec::new(),
            observed_paths: Vec::new(),
            failure: Some(PortFailure::new(FailureCategory::Internal)),
        }
    }
}

impl ValidationCommandDiscovery for FakeDiscovery {
    fn discover_candidates(
        &mut self,
        worktree_path: &Path,
    ) -> Result<Vec<ValidationCommandCandidate>, PortFailure> {
        self.observed_paths.push(worktree_path.to_path_buf());
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        Ok(self.candidates.clone())
    }
}

struct FakeGuard(DirectoryIdentity);

impl DirectoryIdentityGuard for FakeGuard {
    fn identity(&self) -> &DirectoryIdentity {
        &self.0
    }
}

/// Echoes back the queried path as `canonical_path` and a fixed, distinct
/// identity per kind (directory vs. file), so tests can control mismatches
/// by mutating `file_id_hex`/`fail_file`/`fail_directory` directly between
/// calls. `fail_directory_paths` additionally fails inspection for one
/// specific directory path only, so a test can simulate a failure isolated
/// to a single approved `CARGO_HOME`/`RUSTUP_HOME` without also breaking the
/// executable/tool-directory/worktree lookups that share this same fake.
struct FakeFilesystemIdentity {
    directory_volume_serial_hex: String,
    directory_file_id_hex: String,
    file_volume_serial_hex: String,
    file_id_hex: String,
    fail_directory: bool,
    fail_file: bool,
    fail_directory_paths: std::collections::HashSet<PathBuf>,
}

impl Default for FakeFilesystemIdentity {
    fn default() -> Self {
        Self {
            directory_volume_serial_hex: "0000000000000001".to_owned(),
            directory_file_id_hex: "00000000000000000000000000000001".to_owned(),
            file_volume_serial_hex: "0000000000000002".to_owned(),
            file_id_hex: "00000000000000000000000000000002".to_owned(),
            fail_directory: false,
            fail_file: false,
            fail_directory_paths: std::collections::HashSet::new(),
        }
    }
}

impl FilesystemIdentityPort for FakeFilesystemIdentity {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        if self.fail_directory || self.fail_directory_paths.contains(path) {
            return Err(PortFailure::new(FailureCategory::NotFound));
        }
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: self.directory_volume_serial_hex.clone(),
            file_id_hex: self.directory_file_id_hex.clone(),
        })
    }

    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Ok(Box::new(FakeGuard(self.inspect_supported_directory(path)?)))
    }

    fn inspect_supported_file(&mut self, path: &Path) -> Result<DirectoryIdentity, PortFailure> {
        if self.fail_file {
            return Err(PortFailure::new(FailureCategory::NotFound));
        }
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: self.file_volume_serial_hex.clone(),
            file_id_hex: self.file_id_hex.clone(),
        })
    }
}

fn test_candidate() -> ValidationCommandCandidate {
    ValidationCommandCandidate {
        kind: ValidationCommandKind::Test,
        executable: "cargo".to_owned(),
        arguments: vec!["test".to_owned(), "--workspace".to_owned()],
    }
}

/// Outside the `C:/managed/task` worktree used by [`setup_task`].
fn outside_worktree_executable_path() -> PathBuf {
    PathBuf::from("C:/tools/cargo/bin/cargo.exe")
}

fn worktree_ready_isolation(task_id: TaskId, expected_version: u64) -> TaskGitIsolation {
    TaskGitIsolation {
        task_id,
        project_id: ProjectId::new(),
        status: GitIsolationStatus::WorktreeReady,
        operation_id: Some(GitOperationId::new()),
        expected_task_version: expected_version,
        base_branch: Some("main".to_owned()),
        base_commit: Some("a".repeat(40)),
        worktree_path: Some("C:/managed/task".to_owned()),
        branch_created_by_app: true,
        worktree_created_by_app: true,
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

/// A task in `state` (either `Implementing` or `Testing`, the two states
/// this Unit's flow may run in) with a matching `WorktreeReady` isolation
/// record.
fn setup_task(state: TaskState, version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(state, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, version));
    repository.seed_task(task, history);
    (repository, task_id)
}

fn setup_project_root_task(
    state: TaskState,
    version: u64,
    confirmed: bool,
) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(state, version, 20, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let mut repository = FakeRepository::default();
    let mut isolation = worktree_ready_isolation(task_id, version);
    isolation.project_id = project_id;
    repository.isolations.insert(task_id, isolation);
    repository.project_records.insert(
        project_id,
        ProjectRecord {
            id: project_id,
            name: "project".to_owned(),
            root_path: "C:/projects/root".to_owned(),
            canonical_path_key: "c:/projects/root".to_owned(),
            display_path: "C:/projects/root".to_owned(),
            created_at_ms: 1,
            updated_at_ms: 1,
        },
    );
    repository.project_identities.insert(
        project_id,
        ProjectFilesystemIdentityRecord {
            project_id,
            root_volume_serial_hex: "0000000000000001".to_owned(),
            root_file_id_hex: "00000000000000000000000000000001".to_owned(),
            repository_kind: chatoms_ports::git::RepositoryKind::Git,
            git_common_volume_serial_hex: None,
            git_common_file_id_hex: None,
            confirmed,
            revision: 7,
            verified_at_ms: 1,
        },
    );
    repository.seed_task(task, history);
    (repository, task_id)
}

fn approve_test_candidate_request(
    task_id: TaskId,
    version: u64,
    path: PathBuf,
) -> ApproveValidationCommandRequest {
    ApproveValidationCommandRequest::new(
        task_id,
        version,
        ValidationCommandKind::Test,
        "cargo".to_owned(),
        vec!["test".to_owned(), "--workspace".to_owned()],
        path,
        None,
        None,
    )
}

fn approve_project_root_test_request(
    task_id: TaskId,
    project_id: ProjectId,
    version: u64,
) -> ApproveProjectRootValidationCommandRequest {
    ApproveProjectRootValidationCommandRequest::new(
        task_id,
        version,
        project_id,
        ValidationCommandKind::Test,
        "cargo".to_owned(),
        vec!["test".to_owned(), "--workspace".to_owned()],
        outside_worktree_executable_path(),
        None,
        None,
    )
}

#[test]
fn project_root_approval_is_separate_and_only_allowed_while_awaiting_user_diff_approval() {
    let (mut repository, task_id) =
        setup_project_root_task(TaskState::AwaitingUserDiffApproval, 3, true);
    let project_id = repository.tasks[&task_id].project_id();
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let approval =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_project_root_command(approve_project_root_test_request(task_id, project_id, 3))
            .expect("ProjectRoot approval succeeds at the exact pre-merge state");

    assert_eq!(
        approval.execution_scope,
        chatoms_domain::ValidationExecutionScope::ProjectRoot
    );
    assert_eq!(approval.target_project_id, Some(project_id));
    assert!(repository.validation_command_approvals.is_empty());
    assert_eq!(repository.project_root_validation_approvals.len(), 1);

    let (mut wrong_state_repository, wrong_task_id) =
        setup_project_root_task(TaskState::Testing, 3, true);
    let wrong_project_id = wrong_state_repository.tasks[&wrong_task_id].project_id();
    let error = ValidationCommandService::new(
        &mut wrong_state_repository,
        &mut time,
        &mut discovery,
        &mut filesystem,
    )
    .approve_project_root_command(approve_project_root_test_request(
        wrong_task_id,
        wrong_project_id,
        3,
    ))
    .expect_err("Testing cannot create a ProjectRoot approval");
    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
}

#[test]
fn project_root_approval_rejects_stale_unconfirmed_or_mismatched_identity() {
    let (mut stale_repository, stale_task_id) =
        setup_project_root_task(TaskState::AwaitingUserDiffApproval, 3, true);
    let stale_project_id = stale_repository.tasks[&stale_task_id].project_id();
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();
    let stale = ValidationCommandService::new(
        &mut stale_repository,
        &mut time,
        &mut discovery,
        &mut filesystem,
    )
    .approve_project_root_command(approve_project_root_test_request(
        stale_task_id,
        stale_project_id,
        2,
    ))
    .expect_err("stale task version is rejected");
    assert_eq!(stale.code(), ApplicationErrorCode::VersionConflict);

    let (mut unconfirmed_repository, unconfirmed_task_id) =
        setup_project_root_task(TaskState::AwaitingUserDiffApproval, 3, false);
    let unconfirmed_project_id = unconfirmed_repository.tasks[&unconfirmed_task_id].project_id();
    let unconfirmed = ValidationCommandService::new(
        &mut unconfirmed_repository,
        &mut time,
        &mut discovery,
        &mut filesystem,
    )
    .approve_project_root_command(approve_project_root_test_request(
        unconfirmed_task_id,
        unconfirmed_project_id,
        3,
    ))
    .expect_err("unconfirmed project identity is rejected");
    assert_eq!(unconfirmed.code(), ApplicationErrorCode::Internal);

    let (mut mismatch_repository, mismatch_task_id) =
        setup_project_root_task(TaskState::AwaitingUserDiffApproval, 3, true);
    let mismatch_project_id = mismatch_repository.tasks[&mismatch_task_id].project_id();
    mismatch_repository
        .project_identities
        .get_mut(&mismatch_project_id)
        .expect("identity")
        .root_file_id_hex = "ffffffffffffffffffffffffffffffff".to_owned();
    let mismatch = ValidationCommandService::new(
        &mut mismatch_repository,
        &mut time,
        &mut discovery,
        &mut filesystem,
    )
    .approve_project_root_command(approve_project_root_test_request(
        mismatch_task_id,
        mismatch_project_id,
        3,
    ))
    .expect_err("live root identity mismatch is rejected");
    assert_eq!(mismatch.code(), ApplicationErrorCode::Internal);
}

/// Outside the `C:/managed/task` worktree, distinct from
/// [`outside_worktree_executable_path`]'s parent directory.
fn outside_worktree_cargo_home_path() -> PathBuf {
    PathBuf::from("C:/tools/cargo-home")
}

#[test]
fn list_candidates_returns_exactly_what_discovery_proposes_for_the_task_worktree() {
    let (mut repository, task_id) = setup_task(TaskState::Implementing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let candidates =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .list_candidates(task_id, 3)
            .expect("list_candidates succeeds");

    assert_eq!(candidates, vec![test_candidate()]);
    assert_eq!(
        discovery.observed_paths,
        vec![PathBuf::from("C:/managed/task")]
    );
}

#[test]
fn list_candidates_rejects_a_task_that_is_not_implementing_or_testing() {
    let (mut repository, task_id) = setup_task(TaskState::AwaitingDesignApproval, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .list_candidates(task_id, 3)
            .expect_err("AwaitingDesignApproval is not a valid state for this flow");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        discovery.observed_paths.is_empty(),
        "discovery must not run"
    );
}

#[test]
fn list_candidates_rejects_a_stale_task_version() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 5);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .list_candidates(task_id, 4)
            .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn approve_command_persists_an_exact_candidate_match_with_its_executable_binding() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let approval =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                outside_worktree_executable_path(),
            ))
            .expect("approve_command succeeds for a discovered candidate with a valid binding");

    assert_eq!(approval.task_id, task_id);
    assert_eq!(approval.approved_task_version, 3);
    assert_eq!(approval.kind, ValidationCommandKind::Test);
    assert_eq!(approval.approved_at_ms, 30);
    assert_eq!(
        approval.approved_executable_path,
        "C:/tools/cargo/bin/cargo.exe"
    );
    assert_eq!(approval.executable_volume_serial_hex, "0000000000000002");
    assert_eq!(
        approval.executable_file_id_hex,
        "00000000000000000000000000000002"
    );
    assert_eq!(approval.tool_directory_path, "C:/tools/cargo/bin");
    assert_eq!(
        approval.tool_directory_volume_serial_hex,
        "0000000000000001"
    );
    assert_eq!(
        approval.tool_directory_file_id_hex,
        "00000000000000000000000000000001"
    );
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Testing);
    assert_eq!(repository.tasks[&task_id].version(), 3);
    assert!(repository.validation_command_approvals.contains_key(&(
        task_id,
        3,
        ValidationCommandKind::Test
    )));
}

#[test]
fn approve_command_persists_an_approved_cargo_and_rustup_home_binding() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let approval =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                task_id,
                3,
                ValidationCommandKind::Test,
                "cargo".to_owned(),
                vec!["test".to_owned(), "--workspace".to_owned()],
                outside_worktree_executable_path(),
                Some(outside_worktree_cargo_home_path()),
                Some(PathBuf::from("C:/tools/rustup-home")),
            ))
            .expect("approve_command succeeds with an approved cargo/rustup home binding");

    assert_eq!(
        approval.approved_cargo_home_path,
        Some("C:/tools/cargo-home".to_owned())
    );
    assert_eq!(
        approval.cargo_home_volume_serial_hex,
        Some("0000000000000001".to_owned())
    );
    assert_eq!(
        approval.cargo_home_file_id_hex,
        Some("00000000000000000000000000000001".to_owned())
    );
    assert_eq!(
        approval.approved_rustup_home_path,
        Some("C:/tools/rustup-home".to_owned())
    );
}

#[test]
fn approve_command_rejects_a_relative_cargo_home_path() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                task_id,
                3,
                ValidationCommandKind::Test,
                "cargo".to_owned(),
                vec!["test".to_owned(), "--workspace".to_owned()],
                outside_worktree_executable_path(),
                Some(PathBuf::from("cargo-home")),
                None,
            ))
            .expect_err("a relative CARGO_HOME path must never be trusted");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_rejects_a_cargo_home_path_inside_the_task_worktree() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                task_id,
                3,
                ValidationCommandKind::Test,
                "cargo".to_owned(),
                vec!["test".to_owned(), "--workspace".to_owned()],
                outside_worktree_executable_path(),
                Some(PathBuf::from("C:/managed/task/vendored-cargo-home")),
                None,
            ))
            .expect_err("a CARGO_HOME inside the task worktree must never be trusted");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_propagates_a_cargo_home_identity_failure_without_writing_anything() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity {
        fail_directory_paths: std::collections::HashSet::from([outside_worktree_cargo_home_path()]),
        ..FakeFilesystemIdentity::default()
    };

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                task_id,
                3,
                ValidationCommandKind::Test,
                "cargo".to_owned(),
                vec!["test".to_owned(), "--workspace".to_owned()],
                outside_worktree_executable_path(),
                Some(outside_worktree_cargo_home_path()),
                None,
            ))
            .expect_err("a CARGO_HOME identity failure must surface, not be swallowed");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_rejects_a_command_discovery_does_not_currently_propose() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                task_id,
                3,
                ValidationCommandKind::Build,
                "cargo".to_owned(),
                vec!["build".to_owned()],
                outside_worktree_executable_path(),
                None,
                None,
            ))
            .expect_err("Build was never discovered, only Test was");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(
        repository.validation_command_approvals.is_empty(),
        "no approval may be recorded"
    );
}

#[test]
fn approve_command_rejects_an_argv_that_does_not_exactly_match_the_discovered_candidate() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                task_id,
                3,
                ValidationCommandKind::Test,
                "cargo".to_owned(),
                vec![
                    "test".to_owned(),
                    "--workspace".to_owned(),
                    "--release".to_owned(),
                ],
                outside_worktree_executable_path(),
                None,
                None,
            ))
            .expect_err("an extra argument not in the discovered candidate must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_rejects_a_task_that_is_not_implementing_or_testing() {
    let (mut repository, task_id) = setup_task(TaskState::AwaitingDesignApproval, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                outside_worktree_executable_path(),
            ))
            .expect_err("AwaitingDesignApproval is not a valid state for this flow");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_rejects_a_stale_task_version() {
    let (mut repository, task_id) = setup_task(TaskState::Implementing, 5);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                4,
                outside_worktree_executable_path(),
            ))
            .expect_err("stale version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn approve_command_rejects_a_relative_executable_path() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                PathBuf::from("cargo.exe"),
            ))
            .expect_err("a relative executable path must never be trusted");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_rejects_an_executable_path_inside_the_task_worktree() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                PathBuf::from("C:/managed/task/vendored-tools/cargo.exe"),
            ))
            .expect_err("an executable inside the task worktree must never be trusted");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_propagates_a_filesystem_identity_failure_without_writing_anything() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity {
        fail_file: true,
        ..FakeFilesystemIdentity::default()
    };

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                outside_worktree_executable_path(),
            ))
            .expect_err("a filesystem identity failure must surface, not be swallowed");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn approve_command_rejects_a_duplicate_for_the_same_kind() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
        .approve_command(approve_test_candidate_request(
            task_id,
            3,
            outside_worktree_executable_path(),
        ))
        .expect("first approval succeeds");

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                outside_worktree_executable_path(),
            ))
            .expect_err("a second approval for the same (task, version, kind) must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
}

#[test]
fn approve_command_propagates_a_discovery_failure_without_writing_anything() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::failing();
    let mut filesystem = FakeFilesystemIdentity::default();

    let error =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(approve_test_candidate_request(
                task_id,
                3,
                outside_worktree_executable_path(),
            ))
            .expect_err("a discovery failure must surface, not be swallowed");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert!(repository.validation_command_approvals.is_empty());
}

#[test]
fn verify_binding_reports_not_found_when_nothing_has_been_approved() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    let status =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .verify_binding(task_id, 3, ValidationCommandKind::Test)
            .expect("verify_binding succeeds");

    assert_eq!(status, ValidationCommandBindingStatus::NotFound);
}

#[test]
fn verify_binding_reports_verified_when_identity_still_matches() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
        .approve_command(approve_test_candidate_request(
            task_id,
            3,
            outside_worktree_executable_path(),
        ))
        .expect("approval succeeds");

    let status =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .verify_binding(task_id, 3, ValidationCommandKind::Test)
            .expect("verify_binding succeeds");

    assert_eq!(status, ValidationCommandBindingStatus::Verified);
}

#[test]
fn verify_binding_reports_identity_mismatch_when_the_file_identity_changed() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
        .approve_command(approve_test_candidate_request(
            task_id,
            3,
            outside_worktree_executable_path(),
        ))
        .expect("approval succeeds");

    // The file at the same approved path has since been replaced.
    filesystem.file_id_hex = "00000000000000000000000000000099".to_owned();

    let status =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .verify_binding(task_id, 3, ValidationCommandKind::Test)
            .expect("verify_binding succeeds even on a mismatch");

    assert_eq!(status, ValidationCommandBindingStatus::IdentityMismatch);
}

#[test]
fn verify_binding_reports_identity_mismatch_when_the_file_can_no_longer_be_inspected() {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
        .approve_command(approve_test_candidate_request(
            task_id,
            3,
            outside_worktree_executable_path(),
        ))
        .expect("approval succeeds");

    filesystem.fail_file = true;

    let status =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .verify_binding(task_id, 3, ValidationCommandKind::Test)
            .expect("verify_binding succeeds even when re-inspection fails");

    assert_eq!(status, ValidationCommandBindingStatus::IdentityMismatch);
}

#[test]
fn verify_binding_reports_identity_mismatch_when_the_approved_cargo_home_can_no_longer_be_inspected()
 {
    let (mut repository, task_id) = setup_task(TaskState::Testing, 3);
    let mut time = FakeTime::at(30);
    let mut discovery = FakeDiscovery::with_candidates(vec![test_candidate()]);
    let mut filesystem = FakeFilesystemIdentity::default();

    ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
        .approve_command(ApproveValidationCommandRequest::new(
            task_id,
            3,
            ValidationCommandKind::Test,
            "cargo".to_owned(),
            vec!["test".to_owned(), "--workspace".to_owned()],
            outside_worktree_executable_path(),
            Some(outside_worktree_cargo_home_path()),
            None,
        ))
        .expect("approval with a cargo home binding succeeds");

    // The approved CARGO_HOME directory has since become uninspectable
    // (e.g. deleted, or replaced by a reparse point).
    filesystem
        .fail_directory_paths
        .insert(outside_worktree_cargo_home_path());

    let status =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .verify_binding(task_id, 3, ValidationCommandKind::Test)
            .expect("verify_binding succeeds even when re-inspection fails");

    assert_eq!(status, ValidationCommandBindingStatus::IdentityMismatch);
}
