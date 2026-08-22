//! Reachability coverage for the merge lifecycle: every gate a task passes
//! on the way from `AwaitingUserDiffApproval` to `Merging`, `MergeConflict`
//! and back must accept a task that got there through *real* state
//! transitions.
//!
//! Every other test in this crate builds its fixture by asserting a state
//! and a version directly, and then stamps that same version onto the
//! task's `TaskGitIsolation` record. That hides an entire class of defect:
//! `TaskGitIsolation.expected_task_version` is the optimistic-concurrency
//! value of the *isolation* lifecycle and is frozen once the isolation
//! reaches `WorktreeReady`, so in production it is many versions behind the
//! task by the time the task reaches `AwaitingUserDiffApproval`. Gates that
//! compared the two could never hold, which made the whole merge lifecycle
//! unreachable while every fixture-shaped test still passed.
//!
//! So these tests build the task with `support::task_through`, which
//! applies each transition through the domain, and read the isolation's
//! frozen version back out of the resulting history. `versions_really_diverge`
//! asserts that the two really are different, so this file cannot
//! degenerate into the fixture shape it exists to rule out.

mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    manual_merge_resolution::{
        ConfirmManualMergeResolutionRequest, ManualMergeResolutionConfirmationService,
    },
    merge_abort::verify_abort_preconditions,
    merge_conflict_inspection::MergeConflictInspectionService,
    merge_execution::{BeginMergeExecutionRequest, MergeExecutionStarter},
    validation_commands::{ApproveProjectRootValidationCommandRequest, ValidationCommandService},
};
use chatoms_domain::{
    ProjectId, Task, TaskId, TaskState, TaskStateTransition, ValidationCommandKind,
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
    merge_conflict_inspection::{
        MergeConflictInspectionOutcome, MergeConflictInspectionPort,
        MergeConflictInspectionRequest, MergeConflictInspectionResult,
    },
    repository::{
        DiffApprovalRecord, GitIsolationStatus, ProjectFilesystemIdentityRecord, ProjectRecord,
        TaskGitIsolation, ValidationCommandApprovalRecord,
    },
    validation::{ValidationCommandCandidate, ValidationCommandDiscovery},
};

use support::{FakeRepository, FakeTime, task_through, version_on_entering};

const ROOT_PATH: &str = "C:/projects/root";
const COMMON_PATH: &str = "C:/projects/root/.git";
const WORKTREE_PATH: &str = "C:/managed/task";
const TOOL_DIRECTORY_PATH: &str = "C:/tools/cargo/bin";
const CARGO_PATH: &str = "C:/tools/cargo/bin/cargo.exe";

/// The real chain a task walks before a user can approve its diff. Nothing
/// in it is merge-specific — it is simply how a task actually reaches
/// `AwaitingUserDiffApproval`, and it is what pushes the task's version
/// well past the isolation's frozen one.
const CHAIN_TO_AWAITING_DIFF_APPROVAL: &[TaskState] = &[
    TaskState::ProjectValidated,
    TaskState::WorktreeCreating,
    TaskState::WorktreeReady,
    TaskState::Planning,
    TaskState::AwaitingDesignApproval,
    TaskState::Implementing,
    TaskState::Testing,
    TaskState::Reviewing,
    TaskState::AwaitingUserDiffApproval,
];

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
            TOOL_DIRECTORY_PATH => Ok(identity(
                TOOL_DIRECTORY_PATH,
                "00000000000000000000000000000004",
            )),
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

    fn inspect_supported_file(&mut self, path: &Path) -> Result<DirectoryIdentity, PortFailure> {
        if path.to_string_lossy().replace('\\', "/") == CARGO_PATH {
            return Ok(identity(CARGO_PATH, "00000000000000000000000000000005"));
        }
        Err(PortFailure::new(FailureCategory::NotFound))
    }
}

struct DiscoveryFake;

impl ValidationCommandDiscovery for DiscoveryFake {
    fn discover_candidates(
        &mut self,
        _worktree_path: &Path,
    ) -> Result<Vec<ValidationCommandCandidate>, PortFailure> {
        Ok(vec![
            candidate(ValidationCommandKind::Test, &["test", "--workspace"]),
            candidate(ValidationCommandKind::Build, &["build", "--workspace"]),
        ])
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

struct CandidateFake;

impl ManualMergeResolutionCandidatePort for CandidateFake {
    fn resolution_candidate(
        &mut self,
        _request: &ManualMergeResolutionCandidateRequest,
    ) -> ManualResolutionCandidateOutcome {
        ManualResolutionCandidateOutcome::Ready(ManualResolutionCandidate {
            base_commit: "a".repeat(40),
            task_commit: "b".repeat(40),
            merge_head_commit: "c".repeat(40),
            resolution_digest: ManualResolutionDigest::from_digest_bytes([9; 32]),
        })
    }
}

fn identity(path: &str, file_id_hex: &str) -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: PathBuf::from(path),
        volume_serial_hex: "00000000000000000000000000000000".to_owned(),
        file_id_hex: file_id_hex.to_owned(),
    }
}

fn candidate(kind: ValidationCommandKind, arguments: &[&str]) -> ValidationCommandCandidate {
    ValidationCommandCandidate {
        kind,
        executable: "cargo".to_owned(),
        arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
    }
}

fn hash() -> DiffContentHash {
    DiffContentHash::from_digest_bytes([7; 32])
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
        approved_executable_path: CARGO_PATH.to_owned(),
        executable_volume_serial_hex: "00000000000000000000000000000000".to_owned(),
        executable_file_id_hex: "00000000000000000000000000000005".to_owned(),
        tool_directory_path: TOOL_DIRECTORY_PATH.to_owned(),
        tool_directory_volume_serial_hex: "00000000000000000000000000000000".to_owned(),
        tool_directory_file_id_hex: "00000000000000000000000000000004".to_owned(),
        approved_cargo_home_path: None,
        cargo_home_volume_serial_hex: None,
        cargo_home_file_id_hex: None,
        approved_rustup_home_path: None,
        rustup_home_volume_serial_hex: None,
        rustup_home_file_id_hex: None,
        target_project_id: Some(project_id),
        target_project_identity_revision: Some(1),
        target_root_volume_serial_hex: Some("00000000000000000000000000000000".to_owned()),
        target_root_file_id_hex: Some("00000000000000000000000000000001".to_owned()),
        approved_at_ms: 100,
    }
}

/// Seeds a repository whose task reached `states.last()` by actually
/// walking `states`, with an isolation record frozen at the version the
/// task had when it entered `WorktreeReady`.
fn seeded(states: &[TaskState]) -> (FakeRepository, Task, Vec<TaskStateTransition>) {
    seeded_with(states, true)
}

/// As [`seeded`], but `pre_approve_project_root` controls whether the two
/// `ProjectRoot` validation approvals already exist. Tests that exercise
/// recording an approval need them absent, since those rows are immutable
/// and a duplicate is rejected.
fn seeded_with(
    states: &[TaskState],
    pre_approve_project_root: bool,
) -> (FakeRepository, Task, Vec<TaskStateTransition>) {
    let (task, history) = task_through(states);
    let task_id = task.id();
    let project_id = task.project_id();
    let worktree_ready_version = version_on_entering(&history, TaskState::WorktreeReady);
    let approval_version = version_on_entering(&history, TaskState::AwaitingUserDiffApproval);

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
            // Frozen when the isolation completed, exactly as production
            // leaves it. Never re-stamped to the task's current version.
            expected_task_version: worktree_ready_version,
            base_branch: Some("main".to_owned()),
            base_commit: Some("a".repeat(40)),
            worktree_path: Some(WORKTREE_PATH.to_owned()),
            branch_created_by_app: true,
            worktree_created_by_app: true,
            created_at_ms: 1,
            updated_at_ms: 2,
        },
    );
    repository.diff_approvals.insert(
        (task_id, approval_version, hash()),
        DiffApprovalRecord {
            task_id,
            approved_task_version: approval_version,
            diff_content_hash: hash(),
            approved_at_ms: 100,
        },
    );
    if pre_approve_project_root {
        for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
            repository.project_root_validation_approvals.insert(
                (task_id, approval_version, kind),
                project_root_approval(task_id, project_id, approval_version, kind),
            );
        }
    }
    repository.seed_task(task.clone(), history.clone());
    (repository, task, history)
}

/// Appends `rounds` full conflict rounds (`Merging -> MergeConflict ->
/// Merging -> MergeConflict ...`) to the base chain, ending in
/// `MergeConflict`.
fn chain_with_conflict_rounds(rounds: usize) -> Vec<TaskState> {
    let mut states = CHAIN_TO_AWAITING_DIFF_APPROVAL.to_vec();
    for _ in 0..rounds {
        states.push(TaskState::Merging);
        states.push(TaskState::MergeConflict);
    }
    states
}

#[test]
fn versions_really_diverge() {
    let (repository, task, history) = seeded(CHAIN_TO_AWAITING_DIFF_APPROVAL);
    let isolation_version = repository.isolations[&task.id()].expected_task_version;
    let approval_version = version_on_entering(&history, TaskState::AwaitingUserDiffApproval);

    assert_eq!(task.state(), TaskState::AwaitingUserDiffApproval);
    assert_eq!(task.version(), approval_version);
    assert!(
        isolation_version < task.version(),
        "the isolation's frozen version must be behind the task's current version, \
         otherwise these tests would not exercise the defect they exist for"
    );
}

#[test]
fn project_root_candidates_are_listable_while_awaiting_user_diff_approval() {
    let (mut repository, task, _) = seeded(CHAIN_TO_AWAITING_DIFF_APPROVAL);
    let mut time = FakeTime::at(200);
    let mut discovery = DiscoveryFake;
    let mut filesystem = FilesystemFake;

    let candidates =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .list_project_root_candidates(task.id(), task.version())
            .expect("ProjectRoot candidates must be listable in the only state that approves them");

    assert_eq!(candidates.len(), 2);
}

#[test]
fn project_root_approval_succeeds_after_a_real_transition_chain() {
    let (mut repository, task, _) = seeded_with(CHAIN_TO_AWAITING_DIFF_APPROVAL, false);
    let project_id = task.project_id();
    let mut time = FakeTime::at(200);
    let mut discovery = DiscoveryFake;
    let mut filesystem = FilesystemFake;

    let approval =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_project_root_command(ApproveProjectRootValidationCommandRequest::new(
                task.id(),
                task.version(),
                project_id,
                ValidationCommandKind::Test,
                "cargo".to_owned(),
                vec!["test".to_owned(), "--workspace".to_owned()],
                PathBuf::from(CARGO_PATH),
                None,
                None,
            ))
            .expect("ProjectRoot approval must be reachable for a really-transitioned task");

    assert_eq!(
        approval.execution_scope,
        ValidationExecutionScope::ProjectRoot
    );
    assert_eq!(approval.approved_task_version, task.version());
    assert!(
        repository.validation_command_approvals.is_empty(),
        "a ProjectRoot approval must never be stored as a TaskWorktree approval"
    );
}

#[test]
fn merge_start_preflight_accepts_a_frozen_isolation_version() {
    let (mut repository, task, _) = seeded(CHAIN_TO_AWAITING_DIFF_APPROVAL);
    let mut time = FakeTime::at(200);
    let mut filesystem = FilesystemFake;

    let inputs = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(
            task.id(),
            task.version(),
            hash(),
        ))
        .expect("merge start must be reachable for a really-transitioned task");

    assert_eq!(inputs.task.state, TaskState::Merging);
    assert_eq!(inputs.task.version, task.version() + 1);
    assert_eq!(inputs.request.base_branch, "main");
}

#[test]
fn merge_conflict_inspection_reaches_git_after_one_and_after_repeated_conflict_rounds() {
    for rounds in 1..=3 {
        let (mut repository, task, _) = seeded(&chain_with_conflict_rounds(rounds));
        let mut filesystem = FilesystemFake;
        let mut git = InspectionFake { calls: 0 };

        let result =
            MergeConflictInspectionService::new(&mut repository, &mut filesystem, &mut git)
                .inspect(task.id())
                .expect("inspection is read-only and must not fail")
                .expect("a MergeConflict task always yields a result");

        assert_eq!(
            result.outcome,
            MergeConflictInspectionOutcome::ConfirmedUnresolved,
            "round {rounds}: inspection must reach Git, not fail closed as Inconsistent"
        );
        assert_eq!(git.calls, 1, "round {rounds}");
    }
}

#[test]
fn manual_resolution_confirmation_succeeds_after_one_and_after_repeated_conflict_rounds() {
    for rounds in 1..=3 {
        let (mut repository, task, history) = seeded(&chain_with_conflict_rounds(rounds));
        let approval_version = version_on_entering(&history, TaskState::AwaitingUserDiffApproval);
        let mut time = FakeTime::at(300);
        let mut filesystem = FilesystemFake;
        let mut candidate = CandidateFake;

        let view = ManualMergeResolutionConfirmationService::new(
            &mut repository,
            &mut time,
            &mut filesystem,
            &mut candidate,
        )
        .confirm(ConfirmManualMergeResolutionRequest::new(
            task.id(),
            task.version(),
        ))
        .expect("manual resolution confirmation must be reachable");

        assert_eq!(
            view.merge_conflict_task_version,
            task.version(),
            "round {rounds}"
        );
        assert_eq!(
            view.source_approval_task_version, approval_version,
            "round {rounds}: provenance must resolve to the first AwaitingUserDiffApproval version"
        );
    }
}

#[test]
fn merge_abort_preflight_succeeds_after_one_and_after_repeated_conflict_rounds() {
    for rounds in 1..=3 {
        let (mut repository, task, history) = seeded(&chain_with_conflict_rounds(rounds));
        let approval_version = version_on_entering(&history, TaskState::AwaitingUserDiffApproval);
        let mut filesystem = FilesystemFake;

        let preflight =
            verify_abort_preconditions(&mut repository, &mut filesystem, task.id(), task.version())
                .expect("merge abort preflight must be reachable");

        assert_eq!(
            preflight.source_approval_task_version, approval_version,
            "round {rounds}"
        );
        assert_eq!(preflight.base_branch, "main", "round {rounds}");
    }
}
