mod support;

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

use chatoms_application::{
    post_merge_validation::{BeginPostMergeValidationRequest, PostMergeValidationStarter},
    post_merge_validation_execution::PostMergeValidationRecorder,
    tasks::{
        AppendPostMergeValidationResultRequest, FinalizePostMergeValidationBatchRequest,
        TaskService,
    },
};
use chatoms_domain::{
    ActorKind, ReasonCode, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot, ValidationCommandKind, ValidationExecutionScope,
};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    process::{AtomicCancellationSignal, CancellationSignal},
    repository::{
        PostMergeValidationResultOutcome, ProjectFilesystemIdentityRecord, ProjectRecord,
        RepositoryErrorCode, ValidationCommandApprovalRecord,
    },
    validation_execution::{
        ValidationCommandExecutor, ValidationExecutionOutcome, ValidationExecutionRequest,
        ValidationExecutionStartOutcome,
    },
};

use support::{FakeRepository, FakeTime, restored_task};

enum ScriptedOutcome {
    Completed(ValidationExecutionOutcome),
    BindingRejected,
    Panic,
}

struct ScriptedExecutor {
    outcomes: VecDeque<ScriptedOutcome>,
    observed: Vec<ValidationCommandKind>,
}

impl ScriptedExecutor {
    fn new(outcomes: impl IntoIterator<Item = ScriptedOutcome>) -> Self {
        Self {
            outcomes: outcomes.into_iter().collect(),
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
        self.observed.push(request.approval.kind);
        match self.outcomes.pop_front().expect("scripted outcome") {
            ScriptedOutcome::Completed(outcome) => {
                Ok(ValidationExecutionStartOutcome::Completed(outcome))
            }
            ScriptedOutcome::BindingRejected => Ok(
                ValidationExecutionStartOutcome::BindingRejected(
                    chatoms_ports::validation_execution::ValidationBindingRejection::IdentityMismatch,
                ),
            ),
            ScriptedOutcome::Panic => panic!("post-merge executor panic"),
        }
    }
}

fn transition(
    task_id: TaskId,
    sequence: u64,
    from: TaskState,
    to: TaskState,
    task_version: u64,
) -> TaskStateTransition {
    TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id,
        sequence,
        from_state: Some(from),
        to_state: to,
        task_version,
        actor_kind: "test.actor".parse::<ActorKind>().expect("actor"),
        reason_code: "test.reason".parse::<ReasonCode>().expect("reason"),
        occurred_at_ms: 10 + sequence as i64,
    })
    .expect("transition snapshot")
}

fn root_identity() -> DirectoryIdentity {
    DirectoryIdentity {
        canonical_path: PathBuf::from("C:/projects/root"),
        volume_serial_hex: "0000000000000001".to_owned(),
        file_id_hex: "00000000000000000000000000000001".to_owned(),
    }
}

struct RootFilesystem;

impl FilesystemIdentityPort for RootFilesystem {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        if path == root_identity().canonical_path {
            Ok(root_identity())
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

fn approval(
    task_id: TaskId,
    project_id: chatoms_domain::ProjectId,
    approval_version: u64,
    kind: ValidationCommandKind,
) -> ValidationCommandApprovalRecord {
    ValidationCommandApprovalRecord {
        task_id,
        approved_task_version: approval_version,
        execution_scope: ValidationExecutionScope::ProjectRoot,
        kind,
        executable: "cargo".to_owned(),
        arguments: match kind {
            ValidationCommandKind::Test => vec!["test".to_owned(), "--workspace".to_owned()],
            ValidationCommandKind::Build => vec!["build".to_owned(), "--workspace".to_owned()],
            _ => Vec::new(),
        },
        approved_executable_path: "C:/tools/cargo.exe".to_owned(),
        executable_volume_serial_hex: "0000000000000002".to_owned(),
        executable_file_id_hex: "00000000000000000000000000000002".to_owned(),
        tool_directory_path: "C:/tools".to_owned(),
        tool_directory_volume_serial_hex: "0000000000000003".to_owned(),
        tool_directory_file_id_hex: "00000000000000000000000000000003".to_owned(),
        approved_cargo_home_path: None,
        cargo_home_volume_serial_hex: None,
        cargo_home_file_id_hex: None,
        approved_rustup_home_path: None,
        rustup_home_volume_serial_hex: None,
        rustup_home_file_id_hex: None,
        target_project_id: Some(project_id),
        target_project_identity_revision: Some(7),
        target_root_volume_serial_hex: Some(root_identity().volume_serial_hex),
        target_root_file_id_hex: Some(root_identity().file_id_hex),
        approved_at_ms: 10,
    }
}

fn post_merge_repository(approval_version: u64) -> (FakeRepository, TaskId) {
    let (task, _) = restored_task(TaskState::PostMergeTesting, 5, 50, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let history = vec![
        transition(
            task_id,
            1,
            TaskState::Reviewing,
            TaskState::AwaitingUserDiffApproval,
            approval_version,
        ),
        transition(
            task_id,
            2,
            TaskState::AwaitingUserDiffApproval,
            TaskState::Merging,
            4,
        ),
        transition(
            task_id,
            3,
            TaskState::Merging,
            TaskState::PostMergeTesting,
            5,
        ),
    ];
    let mut repository = FakeRepository::default();
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
            root_volume_serial_hex: root_identity().volume_serial_hex,
            root_file_id_hex: root_identity().file_id_hex,
            repository_kind: chatoms_ports::git::RepositoryKind::Git,
            git_common_volume_serial_hex: None,
            git_common_file_id_hex: None,
            confirmed: true,
            revision: 7,
            verified_at_ms: 1,
        },
    );
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        repository.project_root_validation_approvals.insert(
            (task_id, approval_version, kind),
            approval(task_id, project_id, approval_version, kind),
        );
    }
    repository.seed_task(task, history);
    (repository, task_id)
}

/// Mirrors [`post_merge_repository`] but with the conflict-resolved
/// provenance chain: `AwaitingUserDiffApproval -> Merging -> MergeConflict
/// -> Merging -> PostMergeTesting`.
fn post_merge_repository_conflict_resolved(approval_version: u64) -> (FakeRepository, TaskId) {
    let (task, _) = restored_task(TaskState::PostMergeTesting, 7, 50, None);
    let task_id = task.id();
    let project_id = task.project_id();
    let history = vec![
        transition(
            task_id,
            1,
            TaskState::Reviewing,
            TaskState::AwaitingUserDiffApproval,
            approval_version,
        ),
        transition(
            task_id,
            2,
            TaskState::AwaitingUserDiffApproval,
            TaskState::Merging,
            4,
        ),
        transition(task_id, 3, TaskState::Merging, TaskState::MergeConflict, 5),
        transition(task_id, 4, TaskState::MergeConflict, TaskState::Merging, 6),
        transition(
            task_id,
            5,
            TaskState::Merging,
            TaskState::PostMergeTesting,
            7,
        ),
    ];
    let mut repository = FakeRepository::default();
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
            root_volume_serial_hex: root_identity().volume_serial_hex,
            root_file_id_hex: root_identity().file_id_hex,
            repository_kind: chatoms_ports::git::RepositoryKind::Git,
            git_common_volume_serial_hex: None,
            git_common_file_id_hex: None,
            confirmed: true,
            revision: 7,
            verified_at_ms: 1,
        },
    );
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        repository.project_root_validation_approvals.insert(
            (task_id, approval_version, kind),
            approval(task_id, project_id, approval_version, kind),
        );
    }
    repository.seed_task(task, history);
    (repository, task_id)
}

#[test]
fn starter_accepts_the_conflict_resolved_provenance_chain() {
    let (mut repository, task_id) = post_merge_repository_conflict_resolved(3);
    let mut filesystem = RootFilesystem;

    let inputs = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 7))
        .expect("conflict-resolved provenance is accepted");

    assert_eq!(inputs.approval_task_version, 3);
    assert_eq!(inputs.approvals.len(), 2);
}

#[test]
fn starter_binds_project_root_approvals_to_the_merge_chain_source_version() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut filesystem = RootFilesystem;

    let inputs = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 5))
        .expect("matching provenance is accepted");

    assert_eq!(inputs.approval_task_version, 3);
    assert_eq!(inputs.approvals.len(), 2);
    assert_eq!(inputs.target.scope(), ValidationExecutionScope::ProjectRoot);
}

#[test]
fn starter_rejects_approvals_from_a_version_other_than_the_transition_history_source() {
    let (mut repository, task_id) = post_merge_repository(3);
    let project_id = repository.tasks[&task_id].project_id();
    repository.project_root_validation_approvals.clear();
    for kind in [ValidationCommandKind::Test, ValidationCommandKind::Build] {
        repository
            .project_root_validation_approvals
            .insert((task_id, 2, kind), approval(task_id, project_id, 2, kind));
    }
    let mut filesystem = RootFilesystem;

    let error = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 5))
        .expect_err("a different approval version cannot be used as fallback");

    assert_eq!(
        error.code(),
        chatoms_application::error::ApplicationErrorCode::NotFound
    );
}

#[test]
fn recorder_runs_test_then_build_and_completes_atomically() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut filesystem = RootFilesystem;
    let inputs = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 5))
        .expect("matching provenance is accepted");
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new([
        ScriptedOutcome::Completed(ValidationExecutionOutcome::Success),
        ScriptedOutcome::Completed(ValidationExecutionOutcome::Success),
    ]);
    let cancellation = AtomicCancellationSignal::new();

    let view = PostMergeValidationRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(&inputs, &mut executor, &cancellation)
        .expect("the approved Test then Build batch succeeds");

    assert_eq!(
        executor.observed,
        [ValidationCommandKind::Test, ValidationCommandKind::Build]
    );
    assert_eq!(view.state, TaskState::Completed);
    assert_eq!(repository.post_merge_validation_results.len(), 2);
    assert!(repository.active_lease.is_none());
}

#[test]
fn test_failure_records_recovery_and_does_not_start_build() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut filesystem = RootFilesystem;
    let inputs = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 5))
        .expect("matching provenance is accepted");
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new([
        ScriptedOutcome::Completed(ValidationExecutionOutcome::ExitFailure { exit_code: 1 }),
        ScriptedOutcome::Completed(ValidationExecutionOutcome::Success),
    ]);
    let cancellation = AtomicCancellationSignal::new();

    let view = PostMergeValidationRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(&inputs, &mut executor, &cancellation)
        .expect("a confirmed Test failure is recorded");

    assert_eq!(executor.observed, [ValidationCommandKind::Test]);
    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(repository.post_merge_validation_results.len(), 1);
    assert!(repository.active_lease.is_some());
}

#[test]
fn binding_rejection_records_typed_recovery_without_running_build() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut filesystem = RootFilesystem;
    let inputs = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 5))
        .expect("matching provenance is accepted");
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new([ScriptedOutcome::BindingRejected]);
    let cancellation = AtomicCancellationSignal::new();

    let view = PostMergeValidationRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(&inputs, &mut executor, &cancellation)
        .expect("a binding rejection is recorded");

    assert_eq!(executor.observed, [ValidationCommandKind::Test]);
    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert_eq!(
        repository.post_merge_validation_results[0].outcome,
        PostMergeValidationResultOutcome::BindingRejected
    );
    assert!(repository.active_lease.is_some());
}

#[test]
fn executor_panic_is_uncertain_and_recovers_without_running_build() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut filesystem = RootFilesystem;
    let inputs = PostMergeValidationStarter::new(&mut repository, &mut filesystem)
        .begin(BeginPostMergeValidationRequest::new(task_id, 5))
        .expect("matching provenance is accepted");
    let mut time = FakeTime::at(100);
    let mut executor = ScriptedExecutor::new([ScriptedOutcome::Panic]);
    let cancellation = AtomicCancellationSignal::new();

    let view = PostMergeValidationRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(&inputs, &mut executor, &cancellation)
        .expect("panic is reduced to an uncertain recovery result");

    assert_eq!(executor.observed, [ValidationCommandKind::Test]);
    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn final_success_atomically_completes_and_releases_the_lease() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut time = FakeTime::at(100);

    let view = TaskService::new(&mut repository, &mut time)
        .finalize_post_merge_validation_batch(FinalizePostMergeValidationBatchRequest::new(
            task_id,
            3,
            5,
            ValidationCommandKind::Build,
            PostMergeValidationResultOutcome::Success,
            Some(0),
            "post-merge validation succeeded".to_owned(),
            90,
            "application".to_owned(),
            "task.post-merge-validation.result".to_owned(),
        ))
        .expect("final success is stored with Completed");

    assert_eq!(view.state, TaskState::Completed);
    assert!(repository.active_lease.is_none());
    assert_eq!(repository.post_merge_validation_results.len(), 1);
}

#[test]
fn intermediate_success_appends_without_changing_post_merge_state() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut time = FakeTime::at(100);

    let record = TaskService::new(&mut repository, &mut time)
        .append_post_merge_validation_result(AppendPostMergeValidationResultRequest::new(
            task_id,
            3,
            5,
            ValidationCommandKind::Test,
            Some(0),
            "post-merge test succeeded".to_owned(),
            80,
            90,
        ))
        .expect("intermediate success appends");

    assert_eq!(record.attempt_sequence, 1);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::PostMergeTesting
    );
    assert!(repository.active_lease.is_some());
}

#[test]
fn every_non_success_including_cancelled_maps_to_recovery_and_keeps_the_lease() {
    for outcome in [
        PostMergeValidationResultOutcome::ExitFailure,
        PostMergeValidationResultOutcome::TimedOut,
        PostMergeValidationResultOutcome::StdoutBoundExceeded,
        PostMergeValidationResultOutcome::BindingRejected,
        PostMergeValidationResultOutcome::Cancelled,
        PostMergeValidationResultOutcome::Uncertain,
    ] {
        let (mut repository, task_id) = post_merge_repository(3);
        let mut time = FakeTime::at(100);
        let exit_code = (outcome == PostMergeValidationResultOutcome::ExitFailure).then_some(1);

        let view = TaskService::new(&mut repository, &mut time)
            .finalize_post_merge_validation_batch(FinalizePostMergeValidationBatchRequest::new(
                task_id,
                3,
                5,
                ValidationCommandKind::Test,
                outcome,
                exit_code,
                "post-merge validation requires recovery".to_owned(),
                90,
                "application".to_owned(),
                "task.post-merge-validation.result".to_owned(),
            ))
            .expect("fail-closed result is recorded");

        assert_eq!(view.state, TaskState::RecoveryRequired);
        assert!(repository.active_lease.is_some());
    }
}

#[test]
fn primary_and_fallback_persistence_failure_is_never_reported_as_completed() {
    let (mut repository, task_id) = post_merge_repository(3);
    repository.fail_on = Some((
        "finalize_post_merge_validation_batch",
        RepositoryErrorCode::OperationFailed,
    ));
    repository.fail_save_transition_once = Some(RepositoryErrorCode::OperationFailed);
    let mut time = FakeTime::at(100);

    let result = TaskService::new(&mut repository, &mut time).finalize_post_merge_validation_batch(
        FinalizePostMergeValidationBatchRequest::new(
            task_id,
            3,
            5,
            ValidationCommandKind::Build,
            PostMergeValidationResultOutcome::Success,
            Some(0),
            "post-merge validation succeeded".to_owned(),
            90,
            "application".to_owned(),
            "task.post-merge-validation.result".to_owned(),
        ),
    );

    assert!(result.is_err());
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::PostMergeTesting
    );
    assert!(repository.post_merge_validation_results.is_empty());
}

#[test]
fn post_merge_result_read_model_orders_test_before_build_and_keeps_safe_fields() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut time = FakeTime::at(100);

    TaskService::new(&mut repository, &mut time)
        .append_post_merge_validation_result(AppendPostMergeValidationResultRequest::new(
            task_id,
            3,
            5,
            ValidationCommandKind::Test,
            Some(0),
            "safe test summary".to_owned(),
            70,
            80,
        ))
        .expect("Test result is stored");
    TaskService::new(&mut repository, &mut time)
        .finalize_post_merge_validation_batch(FinalizePostMergeValidationBatchRequest::new(
            task_id,
            3,
            5,
            ValidationCommandKind::Build,
            PostMergeValidationResultOutcome::Success,
            Some(0),
            "safe build summary".to_owned(),
            90,
            "application".to_owned(),
            "task.post-merge-validation.result".to_owned(),
        ))
        .expect("Build result is stored");

    let results = TaskService::new(&mut repository, &mut time)
        .get_post_merge_validation_results(task_id)
        .expect("post-merge results are readable");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].kind, ValidationCommandKind::Test);
    assert_eq!(results[0].attempt_sequence, 1);
    assert_eq!(results[0].safe_summary, "safe test summary");
    assert_eq!(results[1].kind, ValidationCommandKind::Build);
    assert_eq!(results[1].attempt_sequence, 1);
    assert_eq!(results[1].safe_summary, "safe build summary");
}

#[test]
fn stale_post_merge_version_writes_neither_result_nor_state() {
    let (mut repository, task_id) = post_merge_repository(3);
    let mut time = FakeTime::at(100);

    let result = TaskService::new(&mut repository, &mut time).finalize_post_merge_validation_batch(
        FinalizePostMergeValidationBatchRequest::new(
            task_id,
            3,
            4,
            ValidationCommandKind::Build,
            PostMergeValidationResultOutcome::Success,
            Some(0),
            "post-merge validation succeeded".to_owned(),
            90,
            "application".to_owned(),
            "task.post-merge-validation.result".to_owned(),
        ),
    );

    assert!(result.is_err());
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::PostMergeTesting
    );
    assert!(repository.post_merge_validation_results.is_empty());
}
