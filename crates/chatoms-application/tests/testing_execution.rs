mod support;

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

use chatoms_application::{
    error::{ApplicationError, ApplicationErrorCode},
    testing_execution::{
        BeginTestingBatchRequest, TestingBatchInputs, TestingBatchRecorder, TestingBatchStarter,
    },
};
use chatoms_domain::{
    GitOperationId, ProjectId, TaskId, TaskState, ValidationCommandKind, ValidationExecutionScope,
};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    process::{AtomicCancellationSignal, CancellationSignal},
    repository::{
        FoundationRepository, GitIsolationStatus, RepositoryErrorCode, TaskGitIsolation,
        ValidationCommandApprovalRecord, ValidationCommandResultOutcome,
    },
    validation_execution::{
        ValidationBindingRejection, ValidationCommandExecutor, ValidationExecutionOutcome,
        ValidationExecutionRequest, ValidationExecutionStartOutcome,
    },
};

use support::{FakeRepository, FakeTime, restored_task};

/// Deliberately looks like a leaked credential so panic-containment tests
/// can assert this exact string never survives into anything the caught
/// panic's containment path returns or records.
const PANIC_SENTINEL: &str = "SIMULATED_EXECUTOR_PANIC_must_never_leak_sk-fake0000000000";

enum ScriptedOutcome {
    Result(Result<ValidationExecutionStartOutcome, PortFailure>),
    Panic,
}

struct ScriptedExecutor {
    scripted: VecDeque<ScriptedOutcome>,
    observed: Vec<(PathBuf, ValidationCommandKind)>,
}

impl ScriptedExecutor {
    fn new(scripted: Vec<ScriptedOutcome>) -> Self {
        Self {
            scripted: scripted.into_iter().collect(),
            observed: Vec::new(),
        }
    }
}

impl ValidationCommandExecutor for ScriptedExecutor {
    fn start_validation_command(
        &mut self,
        request: ValidationExecutionRequest<'_>,
        _cancellation: &dyn CancellationSignal,
    ) -> Result<ValidationExecutionStartOutcome, PortFailure> {
        self.observed.push((
            request.target.directory_identity().canonical_path.clone(),
            request.approval.kind,
        ));
        match self
            .scripted
            .pop_front()
            .expect("a scripted outcome for every call")
        {
            ScriptedOutcome::Result(result) => result,
            ScriptedOutcome::Panic => panic!("{PANIC_SENTINEL}"),
        }
    }
}

fn success() -> ScriptedOutcome {
    ScriptedOutcome::Result(Ok(ValidationExecutionStartOutcome::Completed(
        ValidationExecutionOutcome::Success,
    )))
}

fn exit_failure(exit_code: i32) -> ScriptedOutcome {
    ScriptedOutcome::Result(Ok(ValidationExecutionStartOutcome::Completed(
        ValidationExecutionOutcome::ExitFailure { exit_code },
    )))
}

fn completed(outcome: ValidationExecutionOutcome) -> ScriptedOutcome {
    ScriptedOutcome::Result(Ok(ValidationExecutionStartOutcome::Completed(outcome)))
}

fn binding_rejected(rejection: ValidationBindingRejection) -> ScriptedOutcome {
    ScriptedOutcome::Result(Ok(ValidationExecutionStartOutcome::BindingRejected(
        rejection,
    )))
}

fn genuine_error() -> ScriptedOutcome {
    ScriptedOutcome::Result(Err(PortFailure::new(FailureCategory::Internal)))
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

fn worktree_identity() -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: PathBuf::from("C:/managed/task"),
        volume_serial_hex: "0000000000000009".to_owned(),
        file_id_hex: "00000000000000000000000000000009".to_owned(),
    }
}

struct StaticFilesystem;

impl FilesystemIdentityPort for StaticFilesystem {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        if path == worktree_identity().canonical_path {
            Ok(worktree_identity())
        } else {
            Err(PortFailure::new(FailureCategory::NotFound))
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

fn approval_for(
    task_id: TaskId,
    version: u64,
    kind: ValidationCommandKind,
) -> ValidationCommandApprovalRecord {
    ValidationCommandApprovalRecord {
        task_id,
        approved_task_version: version,
        execution_scope: ValidationExecutionScope::TaskWorktree,
        kind,
        executable: "cargo".to_owned(),
        arguments: vec!["--fixed-argv-for".to_owned(), format!("{kind:?}")],
        approved_executable_path: "C:/tools/cargo/bin/cargo.exe".to_owned(),
        executable_volume_serial_hex: "0000000000000002".to_owned(),
        executable_file_id_hex: "00000000000000000000000000000002".to_owned(),
        tool_directory_path: "C:/tools/cargo/bin".to_owned(),
        tool_directory_volume_serial_hex: "0000000000000001".to_owned(),
        tool_directory_file_id_hex: "00000000000000000000000000000001".to_owned(),
        approved_cargo_home_path: None,
        cargo_home_volume_serial_hex: None,
        cargo_home_file_id_hex: None,
        approved_rustup_home_path: None,
        rustup_home_volume_serial_hex: None,
        rustup_home_file_id_hex: None,
        target_project_id: None,
        target_project_identity_revision: None,
        target_root_volume_serial_hex: None,
        target_root_file_id_hex: None,
        approved_at_ms: 50,
    }
}

/// A task in `Testing` with a matching `WorktreeReady` isolation record and
/// one approval per `kind` in `kinds`, at `version`.
fn setup_testing(version: u64, kinds: &[ValidationCommandKind]) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Testing, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, version));
    for kind in kinds {
        repository.validation_command_approvals.insert(
            (task_id, version, *kind),
            approval_for(task_id, version, *kind),
        );
    }
    repository.seed_task(task, history);
    (repository, task_id)
}

fn approvals_for(
    task_id: TaskId,
    version: u64,
    kinds: &[ValidationCommandKind],
) -> Vec<ValidationCommandApprovalRecord> {
    kinds
        .iter()
        .map(|kind| approval_for(task_id, version, *kind))
        .collect()
}

fn begin_testing(
    repository: &mut FakeRepository,
    task_id: TaskId,
    version: u64,
) -> Result<TestingBatchInputs, ApplicationError> {
    let mut filesystem = StaticFilesystem;
    TestingBatchStarter::new(repository, &mut filesystem)
        .begin(BeginTestingBatchRequest::new(task_id, version))
}

#[test]
fn begin_returns_approvals_ordered_by_the_fixed_kind_sequence_regardless_of_insertion_order() {
    let (mut repository, task_id) = setup_testing(
        3,
        &[
            ValidationCommandKind::Build,
            ValidationCommandKind::Format,
            ValidationCommandKind::Test,
        ],
    );

    let inputs = begin_testing(&mut repository, task_id, 3).expect("begin succeeds");

    assert_eq!(
        inputs
            .approvals
            .iter()
            .map(|approval| approval.kind)
            .collect::<Vec<_>>(),
        vec![
            ValidationCommandKind::Format,
            ValidationCommandKind::Test,
            ValidationCommandKind::Build,
        ]
    );
    assert_eq!(inputs.worktree_identity, worktree_identity());
    assert_eq!(inputs.task.state, TaskState::Testing);
}

#[test]
fn begin_rejects_a_task_that_is_not_testing() {
    let (task, history) = restored_task(TaskState::Implementing, 3, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 3));
    repository.validation_command_approvals.insert(
        (task_id, 3, ValidationCommandKind::Format),
        approval_for(task_id, 3, ValidationCommandKind::Format),
    );
    repository.seed_task(task, history);

    let error = begin_testing(&mut repository, task_id, 3)
        .expect_err("Implementing is not a valid state for a Testing batch");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
}

#[test]
fn begin_rejects_a_stale_task_version() {
    let (mut repository, task_id) = setup_testing(5, &[ValidationCommandKind::Format]);

    let error = begin_testing(&mut repository, task_id, 1)
        .expect_err("stale expected_version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn begin_rejects_a_missing_isolation_record() {
    let (task, history) = restored_task(TaskState::Testing, 3, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.validation_command_approvals.insert(
        (task_id, 3, ValidationCommandKind::Format),
        approval_for(task_id, 3, ValidationCommandKind::Format),
    );
    repository.seed_task(task, history);

    let error = begin_testing(&mut repository, task_id, 3)
        .expect_err("a Testing batch requires a WorktreeReady isolation record");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
}

#[test]
fn begin_rejects_an_empty_approval_set_without_changing_state() {
    let (mut repository, task_id) = setup_testing(3, &[]);

    let error = begin_testing(&mut repository, task_id, 3)
        .expect_err("no approved validation command means the batch cannot run yet");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Testing);
    assert_eq!(repository.tasks[&task_id].version(), 3);
}

#[test]
fn run_and_record_all_success_reaches_reviewing_and_appends_every_intermediate_result() {
    let (mut repository, task_id) = setup_testing(
        3,
        &[ValidationCommandKind::Format, ValidationCommandKind::Test],
    );
    let approvals = approvals_for(
        task_id,
        3,
        &[ValidationCommandKind::Format, ValidationCommandKind::Test],
    );
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![success(), success()]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::Reviewing);
    assert_eq!(
        executor.observed,
        vec![
            (
                PathBuf::from("C:/managed/task"),
                ValidationCommandKind::Format
            ),
            (
                PathBuf::from("C:/managed/task"),
                ValidationCommandKind::Test
            ),
        ]
    );
    let intermediate = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Format)
        .expect("list Format results");
    assert_eq!(intermediate.len(), 1);
    assert_eq!(intermediate[0].attempt_sequence, 1);
    assert_eq!(
        intermediate[0].outcome,
        ValidationCommandResultOutcome::Success
    );
    let last = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Test)
        .expect("list Test results");
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].outcome, ValidationCommandResultOutcome::Success);
    assert!(
        repository.active_lease.is_some(),
        "Reviewing still requires the active lease"
    );
}

#[test]
fn run_and_record_stops_after_the_first_exit_failure_and_never_runs_later_commands() {
    let (mut repository, task_id) = setup_testing(
        3,
        &[
            ValidationCommandKind::Format,
            ValidationCommandKind::Test,
            ValidationCommandKind::Build,
        ],
    );
    let approvals = approvals_for(
        task_id,
        3,
        &[
            ValidationCommandKind::Format,
            ValidationCommandKind::Test,
            ValidationCommandKind::Build,
        ],
    );
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![success(), exit_failure(101)]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("a failure is recorded, not propagated as an error");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(
        executor.observed.len(),
        2,
        "Build must never run after Test fails"
    );
    let test_results = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Test)
        .expect("list Test results");
    assert_eq!(
        test_results[0].outcome,
        ValidationCommandResultOutcome::ExitFailure
    );
    assert_eq!(test_results[0].exit_code, Some(101));
    assert_eq!(
        test_results[0].safe_summary,
        "validation command exited with a nonzero status"
    );
    assert!(
        repository
            .list_validation_command_results(task_id, 3, ValidationCommandKind::Build)
            .expect("list Build results")
            .is_empty(),
        "Build must never receive a result row"
    );
    assert!(repository.active_lease.is_some());
}

#[test]
fn run_and_record_stops_on_the_first_of_every_non_success_confirmed_outcome() {
    for (outcome, safe_summary) in [
        (
            ValidationExecutionOutcome::TimedOut,
            "validation command exceeded its time limit",
        ),
        (
            ValidationExecutionOutcome::StdoutBoundExceeded,
            "validation command output exceeded the allowed size",
        ),
        (
            ValidationExecutionOutcome::Uncertain,
            "validation command outcome could not be confirmed",
        ),
    ] {
        let (mut repository, task_id) = setup_testing(
            3,
            &[ValidationCommandKind::Format, ValidationCommandKind::Test],
        );
        let approvals = approvals_for(
            task_id,
            3,
            &[ValidationCommandKind::Format, ValidationCommandKind::Test],
        );
        let mut time = FakeTime::at(100);
        let mut executor = ScriptedExecutor::new(vec![success(), completed(outcome)]);
        let cancellation = AtomicCancellationSignal::new();

        let view = TestingBatchRecorder::new(&mut repository, &mut time)
            .run_and_record(
                task_id,
                3,
                &worktree_identity(),
                &approvals,
                &mut executor,
                &cancellation,
            )
            .unwrap_or_else(|error| {
                panic!("case {outcome:?} must be recorded, not errored: {error}")
            });

        assert_eq!(view.state, TaskState::RecoveryRequired, "case: {outcome:?}");
        let test_results = repository
            .list_validation_command_results(task_id, 3, ValidationCommandKind::Test)
            .expect("list Test results");
        assert_eq!(test_results[0].exit_code, None, "case: {outcome:?}");
        assert_eq!(
            test_results[0].safe_summary, safe_summary,
            "case: {outcome:?}"
        );
    }
}

#[test]
fn run_and_record_confirmed_cancellation_reaches_paused_with_testing_resume_target() {
    let (mut repository, task_id) = setup_testing(
        3,
        &[ValidationCommandKind::Format, ValidationCommandKind::Test],
    );
    let approvals = approvals_for(
        task_id,
        3,
        &[ValidationCommandKind::Format, ValidationCommandKind::Test],
    );
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![
        success(),
        completed(ValidationExecutionOutcome::Cancelled),
    ]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("a confirmed cancellation is recorded");

    assert_eq!(view.state, TaskState::Paused);
    assert_eq!(view.resume_target_state, Some(TaskState::Testing));
    assert!(
        repository.active_lease.is_some(),
        "Paused must keep the active lease"
    );
    let test_results = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Test)
        .expect("list Test results");
    assert_eq!(
        test_results[0].outcome,
        ValidationCommandResultOutcome::Cancelled
    );
}

#[test]
fn run_and_record_treats_a_genuine_executor_error_as_uncertain_and_falls_back_to_recovery_required()
{
    let (mut repository, task_id) = setup_testing(3, &[ValidationCommandKind::Format]);
    let approvals = approvals_for(task_id, 3, &[ValidationCommandKind::Format]);
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![genuine_error()]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("a genuine executor failure is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    let results = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Format)
        .expect("list results");
    assert_eq!(
        results[0].outcome,
        ValidationCommandResultOutcome::Uncertain
    );
    assert!(repository.active_lease.is_some());
}

#[test]
fn run_and_record_rejects_a_binding_rejection_without_writing_anything() {
    for rejection in [
        ValidationBindingRejection::IdentityMismatch,
        ValidationBindingRejection::ExecutableInsideExecutionTarget,
        ValidationBindingRejection::UnapprovedCommandKind,
    ] {
        let (mut repository, task_id) = setup_testing(3, &[ValidationCommandKind::Format]);
        let approvals = approvals_for(task_id, 3, &[ValidationCommandKind::Format]);
        let mut time = FakeTime::at(100);
        let mut executor = ScriptedExecutor::new(vec![binding_rejected(rejection)]);
        let cancellation = AtomicCancellationSignal::new();

        let error = TestingBatchRecorder::new(&mut repository, &mut time)
            .run_and_record(
                task_id,
                3,
                &worktree_identity(),
                &approvals,
                &mut executor,
                &cancellation,
            )
            .expect_err("a binding rejection must never spawn and must preserve state");

        assert_eq!(
            error.code(),
            ApplicationErrorCode::Internal,
            "case: {rejection:?}"
        );
        assert_eq!(repository.tasks[&task_id].state(), TaskState::Testing);
        assert_eq!(repository.tasks[&task_id].version(), 3);
        assert!(
            repository
                .list_validation_command_results(task_id, 3, ValidationCommandKind::Format)
                .expect("list results")
                .is_empty(),
            "case: {rejection:?}"
        );
    }
}

#[test]
fn run_and_record_rejects_an_empty_approval_slice() {
    let (mut repository, task_id) = setup_testing(3, &[]);
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![]);
    let cancellation = AtomicCancellationSignal::new();

    let error = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            3,
            &worktree_identity(),
            &[],
            &mut executor,
            &cancellation,
        )
        .expect_err("an empty batch must never silently report success");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Testing);
    assert!(executor.observed.is_empty());
}

#[test]
fn finalize_persistence_failure_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_testing(3, &[ValidationCommandKind::Format]);
    let approvals = approvals_for(task_id, 3, &[ValidationCommandKind::Format]);
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![success()]);
    let cancellation = AtomicCancellationSignal::new();
    repository.fail_on = Some((
        "finalize_validation_command_batch",
        RepositoryErrorCode::DatabaseUnavailable,
    ));

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("a rejected primary write still falls back to RecoveryRequired");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn panic_containment_records_an_uncertain_result_for_the_panicking_kind_and_reaches_recovery_required()
 {
    let (mut repository, task_id) = setup_testing(
        3,
        &[ValidationCommandKind::Format, ValidationCommandKind::Test],
    );
    let approvals = approvals_for(
        task_id,
        3,
        &[ValidationCommandKind::Format, ValidationCommandKind::Test],
    );
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![success(), ScriptedOutcome::Panic]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("a contained panic still records RecoveryRequired");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
    let format_results = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Format)
        .expect("list Format results");
    assert_eq!(format_results.len(), 1);
    assert_eq!(
        format_results[0].outcome,
        ValidationCommandResultOutcome::Success
    );
    let test_results = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Test)
        .expect("list Test results");
    assert_eq!(test_results.len(), 1);
    assert_eq!(
        test_results[0].outcome,
        ValidationCommandResultOutcome::Uncertain
    );
    assert_eq!(test_results[0].exit_code, None);
    assert_eq!(
        test_results[0].safe_summary,
        "validation command outcome could not be confirmed"
    );
}

#[test]
fn panic_containment_never_lets_the_panic_payload_reach_anything_recorded() {
    let (mut repository, task_id) = setup_testing(3, &[ValidationCommandKind::Format]);
    let approvals = approvals_for(task_id, 3, &[ValidationCommandKind::Format]);
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![ScriptedOutcome::Panic]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            3,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect("a contained panic still records a result");

    let stored = repository
        .list_validation_command_results(task_id, 3, ValidationCommandKind::Format)
        .expect("list results");
    let rendered = format!("{view:?} {stored:?}");
    assert!(
        !rendered.contains(PANIC_SENTINEL),
        "the panic payload must never surface in the recorded TaskView or result"
    );
    assert!(!rendered.to_lowercase().contains("stdout"));
    assert!(!rendered.to_lowercase().contains("stderr"));
}

#[test]
fn panic_containment_does_not_report_success_when_the_finalize_write_is_itself_rejected() {
    let (mut repository, task_id) = setup_testing(3, &[ValidationCommandKind::Format]);
    let approvals = approvals_for(task_id, 3, &[ValidationCommandKind::Format]);
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![ScriptedOutcome::Panic]);
    let cancellation = AtomicCancellationSignal::new();
    let stale_expected_version = 99;

    let error = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            stale_expected_version,
            &worktree_identity(),
            &approvals,
            &mut executor,
            &cancellation,
        )
        .expect_err("a rejected finalize write must never be reported as success");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::Testing,
        "the task must be left exactly as it was"
    );
    assert!(repository.active_lease.is_some());
    assert!(
        repository
            .list_validation_command_results(task_id, 3, ValidationCommandKind::Format)
            .expect("list results")
            .is_empty(),
        "no result may be recorded when the fallback write itself is rejected"
    );
}

#[test]
fn begin_then_run_and_record_connects_the_starter_ordered_batch_and_the_recorder_end_to_end() {
    let (mut repository, task_id) = setup_testing(
        3,
        &[ValidationCommandKind::Test, ValidationCommandKind::Format],
    );

    let inputs = begin_testing(&mut repository, task_id, 3).expect("begin succeeds");
    assert_eq!(
        inputs
            .approvals
            .iter()
            .map(|approval| approval.kind)
            .collect::<Vec<_>>(),
        vec![ValidationCommandKind::Format, ValidationCommandKind::Test]
    );

    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new(vec![success(), success()]);
    let cancellation = AtomicCancellationSignal::new();

    let view = TestingBatchRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            inputs.task.version,
            &inputs.worktree_identity,
            &inputs.approvals,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::Reviewing);
    assert_eq!(
        executor
            .observed
            .iter()
            .map(|(_, kind)| *kind)
            .collect::<Vec<_>>(),
        vec![ValidationCommandKind::Format, ValidationCommandKind::Test]
    );
}
