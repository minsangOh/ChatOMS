use chatoms_application::{
    error::ApplicationError,
    merge_execution::{BeginMergeExecutionRequest, MergeExecutionRecorder, MergeExecutionStarter},
    post_merge_validation::{BeginPostMergeValidationRequest, PostMergeValidationStarter},
    post_merge_validation_execution::PostMergeValidationRecorder,
    tasks::{RecordMergeResultRequest, TaskActionRequest, TaskService},
    user_diff_approval::{ApproveUserDiffRequest, UserDiffApprovalService},
};
use chatoms_domain::TaskState;
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::{
    diff::DiffContentHash, error::FailureCategory, merge_execution::MergeExecutionOutcome,
    process::AtomicCancellationSignal, repository::FoundationRepository,
};

use crate::{dto::TaskDto, error::IpcErrorDto, state::ManagedRuntime};

use super::tasks::parse_task_id;

const ACTOR_KIND: &str = "user";
const RESULT_REASON: &str = "task.merge.result";

pub fn handle_approve_user_diff_and_start_merge(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
    expected_diff_content_hash: &str,
) -> Result<TaskDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let expected_hash =
        DiffContentHash::from_hex(expected_diff_content_hash).ok_or_else(invalid_hash_error)?;
    let ready = runtime.ready_snapshot()?;
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let mut filesystem = ready.filesystem.clone();

    MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .require_project_root_validation_approvals(id, expected_version)
        .map_err(IpcErrorDto::from)?;

    let mut approval_candidate = GitCliAdapter::from_environment()
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    UserDiffApprovalService::new(
        &mut repository,
        &mut time,
        &mut filesystem,
        &mut approval_candidate,
    )
    .approve(ApproveUserDiffRequest::new(
        id,
        expected_version,
        expected_hash,
    ))
    .map_err(IpcErrorDto::from)?;

    let inputs = MergeExecutionStarter::new(&mut repository, &mut time, &mut filesystem)
        .begin(BeginMergeExecutionRequest::new(
            id,
            expected_version,
            expected_hash,
        ))
        .map_err(IpcErrorDto::from)?;
    let task_dto = TaskDto::from(inputs.task.clone());
    let merge_request = inputs.request;
    let expected_version = inputs.task.version;
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();
    let mut filesystem_for_thread = ready.filesystem.clone();
    let app_temp_dir = ready.app_temp_dir();

    // `Builder::spawn` rather than `thread::spawn`: the latter panics when
    // the OS refuses a new thread, which would unwind out of a Tauri command
    // *after* `MergeExecutionStarter::begin` has already committed
    // `AwaitingUserDiffApproval -> Merging`, stranding the task in `Merging`
    // with its `ActiveTaskLease` held. Here the failure is a value, so the
    // task is moved to `RecoveryRequired` on the way out and the caller gets
    // a typed error carrying no OS detail.
    let spawn_result = std::thread::Builder::new().spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut adapter = match GitCliAdapter::from_environment() {
                Ok(adapter) => adapter,
                Err(_) => {
                    record_uncertain_merge(
                        &mut repository_for_thread,
                        &mut time_for_thread,
                        id,
                        expected_version,
                    );
                    return;
                }
            };
            let result =
                MergeExecutionRecorder::new(&mut repository_for_thread, &mut time_for_thread)
                    .run_and_record_with_panic_containment(
                        id,
                        expected_version,
                        &merge_request,
                        &mut adapter,
                    );
            match result {
                Ok(view) if view.state == TaskState::PostMergeTesting => {
                    run_post_merge_validation(
                        &mut repository_for_thread,
                        &mut time_for_thread,
                        &mut filesystem_for_thread,
                        app_temp_dir,
                        id,
                        view.version,
                    );
                }
                Ok(_) => {}
                Err(_) => record_uncertain_merge(
                    &mut repository_for_thread,
                    &mut time_for_thread,
                    id,
                    expected_version,
                ),
            }
        }));
        if result.is_err() {
            let _ =
                record_uncertain_background(&mut repository_for_thread, &mut time_for_thread, id);
        }
    });
    if spawn_result.is_err() {
        // No Git write can have happened — the thread never ran — but the
        // `Merging` transition is already committed, so the task must not be
        // left as if a merge were in flight. `record_uncertain_background`
        // reads the state and version as they are persisted right now and
        // records the existing fail-closed recovery outcome; if even that
        // fails the task stays non-terminal and keeps its lease, and
        // `TaskService::reconcile_startup_merge` recovers it on the next
        // startup. The raw spawn error is dropped here, never surfaced.
        let mut repository_for_recovery = ready.repository.clone();
        let mut time_for_recovery = ready.time.clone();
        let _ =
            record_uncertain_background(&mut repository_for_recovery, &mut time_for_recovery, id);
        return Err(category_error(FailureCategory::Internal));
    }

    Ok(task_dto)
}

pub(crate) fn run_post_merge_validation(
    repository: &mut crate::state::RepositoryHandle,
    time: &mut crate::state::TimeProviderHandle,
    filesystem: &mut crate::state::FilesystemIdentityHandle,
    app_temp_dir: Option<std::path::PathBuf>,
    task_id: chatoms_domain::TaskId,
    expected_version: u64,
) {
    let inputs = match PostMergeValidationStarter::new(repository, filesystem).begin(
        BeginPostMergeValidationRequest::new(task_id, expected_version),
    ) {
        Ok(inputs) => inputs,
        Err(_) => {
            record_post_merge_recovery(repository, time, task_id, expected_version);
            return;
        }
    };
    let Some(app_temp_dir) = app_temp_dir else {
        record_post_merge_recovery(repository, time, task_id, expected_version);
        return;
    };
    let mut adapter = chatoms_infrastructure::validation_execution::CargoValidationAdapter::new(
        chatoms_infrastructure::process::StdProcessRunner::new(),
        filesystem.clone(),
        app_temp_dir,
    );
    let cancellation = AtomicCancellationSignal::new();
    let result = PostMergeValidationRecorder::new(repository, time)
        .run_and_record_with_panic_containment(&inputs, &mut adapter, &cancellation);
    if result.is_err() {
        record_post_merge_recovery(repository, time, task_id, expected_version);
    }
}

fn record_uncertain_merge(
    repository: &mut crate::state::RepositoryHandle,
    time: &mut crate::state::TimeProviderHandle,
    task_id: chatoms_domain::TaskId,
    expected_version: u64,
) {
    let _ = TaskService::new(repository, time).record_merge_result(RecordMergeResultRequest::new(
        task_id,
        expected_version,
        MergeExecutionOutcome::PostWriteUncertain,
        ACTOR_KIND.to_owned(),
        RESULT_REASON.to_owned(),
    ));
}

pub(crate) fn record_post_merge_recovery(
    repository: &mut crate::state::RepositoryHandle,
    time: &mut crate::state::TimeProviderHandle,
    task_id: chatoms_domain::TaskId,
    expected_version: u64,
) {
    let _ = TaskService::new(repository, time).mark_recovery_required(TaskActionRequest::new(
        task_id,
        expected_version,
        "application".to_owned(),
        "task.post-merge-validation.recovery-required".to_owned(),
    ));
}

/// What [`record_uncertain_background`] actually did. Returned so that "the
/// task could not be read at all" stays distinguishable from "a fail-closed
/// recovery was recorded" instead of collapsing into one silent no-op.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackgroundRecoveryOutcome {
    /// A fail-closed recovery was recorded against the task's currently
    /// persisted version.
    Recorded,
    /// The task is no longer in a stage this fallback owns, so there is
    /// nothing for it to record.
    NotApplicable,
    /// The task could not be read, so nothing was recorded. Never reported
    /// as success: the task stays non-terminal, keeps its
    /// `ActiveTaskLease`, and `TaskService::reconcile_startup_merge`
    /// recovers it on the next startup.
    Unresolved,
}

/// Fail-closed fallback for a panic that escaped the inner containment.
///
/// Such a panic could have happened either inside the merge write itself
/// (task still `Merging`) or inside the post-merge validation path that
/// follows a successful merge (task already `PostMergeTesting`) — the
/// correct recovery call differs by which stage was in flight, and so does
/// the optimistic-concurrency version it must be recorded against. Both are
/// therefore read from the task as it is persisted *now*: the version this
/// background run started with is stale the moment `Merging ->
/// PostMergeTesting` commits, and reusing it would turn the recovery write
/// into a silently swallowed `VersionConflict`.
pub(crate) fn record_uncertain_background(
    repository: &mut crate::state::RepositoryHandle,
    time: &mut crate::state::TimeProviderHandle,
    task_id: chatoms_domain::TaskId,
) -> BackgroundRecoveryOutcome {
    let Ok(Some(task)) = repository.get_task(task_id) else {
        return BackgroundRecoveryOutcome::Unresolved;
    };
    match task.state() {
        TaskState::Merging => {
            record_uncertain_merge(repository, time, task_id, task.version());
            BackgroundRecoveryOutcome::Recorded
        }
        TaskState::PostMergeTesting => {
            record_post_merge_recovery(repository, time, task_id, task.version());
            BackgroundRecoveryOutcome::Recorded
        }
        _ => BackgroundRecoveryOutcome::NotApplicable,
    }
}

fn category_error(category: FailureCategory) -> IpcErrorDto {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
    .into()
}

fn invalid_hash_error() -> IpcErrorDto {
    ApplicationError::from_failure(
        FailureCategory::InvalidInput,
        FailureCategory::InvalidInput.default_severity(),
        FailureCategory::InvalidInput.default_retry(),
    )
    .into()
}

#[tauri::command(rename_all = "camelCase")]
pub fn approve_user_diff_and_start_merge(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
    expected_diff_content_hash: String,
) -> Result<TaskDto, IpcErrorDto> {
    handle_approve_user_diff_and_start_merge(
        &state,
        &task_id,
        expected_version,
        &expected_diff_content_hash,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use chatoms_domain::{
        Task, TaskBranchIdentity, TaskId, TaskSnapshot, TaskState, TaskStateTransition,
    };
    use chatoms_ports::{
        TimeProvider,
        error::PortFailure,
        repository::{
            ActiveLease, FoundationRepository, ProjectSummary, RepositoryError, RepositoryErrorCode,
        },
    };

    use crate::state::{RepositoryHandle, TimeProviderHandle};

    /// Every `save_transition` this fake receives, as
    /// `(expected_version, resulting_state)`. Shared with the test so the
    /// optimistic-concurrency version the recovery path actually used is
    /// observable.
    pub(crate) type SavedTransitions = Arc<Mutex<Vec<(u64, TaskState)>>>;

    /// A `FoundationRepository` that answers only what the fail-closed
    /// recovery path needs, and records the version each write was made
    /// against. Every other method falls through to the trait's
    /// `OperationFailed` default.
    pub(crate) struct RecordingRepository {
        pub(crate) task: Result<Option<Task>, RepositoryErrorCode>,
        pub(crate) saved: SavedTransitions,
    }

    impl FoundationRepository for RecordingRepository {
        fn get_task(&mut self, _task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
            self.task.clone().map_err(RepositoryError::new)
        }

        fn create_task(
            &mut self,
            _task: &Task,
            _initial_transition: &TaskStateTransition,
            _lease_acquired_at_ms: i64,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
        }

        fn save_recovery_target(
            &mut self,
            _expected_version: u64,
            _task: &Task,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
        }

        fn terminate_task(
            &mut self,
            _expected_version: u64,
            _task: &Task,
            _transition: &TaskStateTransition,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
        }

        /// Must be non-empty: `RepositoryHandle` does not override
        /// `next_transition_sequence`, so the trait default runs and derives
        /// the next sequence from the last transition listed here.
        fn list_task_transitions(
            &mut self,
            task_id: TaskId,
        ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
            Ok(vec![chatoms_domain::TaskStateTransition::initial(
                chatoms_domain::TaskStateTransitionId::new(),
                task_id,
                "application".parse().expect("actor"),
                "test.reason".parse().expect("reason"),
                10,
            )])
        }

        fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
            Ok(Vec::new())
        }

        fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
            Ok(None)
        }

        fn save_transition(
            &mut self,
            expected_version: u64,
            task: &Task,
            _transition: &TaskStateTransition,
        ) -> Result<(), RepositoryError> {
            self.saved
                .lock()
                .expect("saved transitions mutex")
                .push((expected_version, task.state()));
            Ok(())
        }
    }

    struct FixedTime;

    impl TimeProvider for FixedTime {
        fn now_ms(&mut self) -> Result<i64, PortFailure> {
            Ok(1_000)
        }
    }

    pub(crate) fn task_at(state: TaskState, version: u64) -> Task {
        let id = TaskId::new();
        Task::restore(TaskSnapshot {
            id,
            project_id: chatoms_domain::ProjectId::new(),
            state,
            version,
            task_branch_identity: TaskBranchIdentity::for_task(id),
            resume_target_state: None,
            created_at_ms: 10,
            updated_at_ms: 10,
            terminal_at_ms: None,
        })
        .expect("test task must satisfy domain invariants")
    }

    pub(crate) fn handles(
        task: Result<Option<Task>, RepositoryErrorCode>,
    ) -> (RepositoryHandle, TimeProviderHandle, SavedTransitions) {
        let saved: SavedTransitions = Arc::new(Mutex::new(Vec::new()));
        (
            RepositoryHandle::new(RecordingRepository {
                task,
                saved: Arc::clone(&saved),
            }),
            TimeProviderHandle::new(FixedTime),
            saved,
        )
    }

    /// The defect this pins down: the background run starts while the task
    /// is `Merging`, but a panic escaping the outer containment *after*
    /// `Merging -> PostMergeTesting` committed must record recovery against
    /// the `PostMergeTesting` version. Reusing the version the run started
    /// with made `save_transition` fail `VersionConflict`, and the caller
    /// discards that error — so the fail-closed recovery silently did
    /// nothing.
    #[test]
    fn post_merge_recovery_uses_the_currently_persisted_version_not_the_starting_one() {
        let started_version = 7;
        let task = task_at(TaskState::PostMergeTesting, started_version + 1);
        let task_id = task.id();
        let (mut repository, mut time, saved) = handles(Ok(Some(task)));

        let outcome = super::record_uncertain_background(&mut repository, &mut time, task_id);

        assert_eq!(outcome, super::BackgroundRecoveryOutcome::Recorded);
        assert_eq!(
            *saved.lock().expect("saved transitions mutex"),
            vec![(started_version + 1, TaskState::RecoveryRequired)],
            "recovery must be written against the version the task actually has now"
        );
    }

    #[test]
    fn merging_recovery_uses_the_currently_persisted_version() {
        let task = task_at(TaskState::Merging, 7);
        let task_id = task.id();
        let (mut repository, mut time, saved) = handles(Ok(Some(task)));

        let outcome = super::record_uncertain_background(&mut repository, &mut time, task_id);

        assert_eq!(outcome, super::BackgroundRecoveryOutcome::Recorded);
        assert_eq!(
            saved
                .lock()
                .expect("saved transitions mutex")
                .first()
                .map(|(version, _)| *version),
            Some(7)
        );
    }

    #[test]
    fn a_failed_task_lookup_is_reported_unresolved_rather_than_silently_succeeding() {
        let (mut repository, mut time, saved) = handles(Err(RepositoryErrorCode::OperationFailed));

        let outcome = super::record_uncertain_background(&mut repository, &mut time, TaskId::new());

        assert_eq!(outcome, super::BackgroundRecoveryOutcome::Unresolved);
        assert!(saved.lock().expect("saved transitions mutex").is_empty());
    }

    #[test]
    fn a_task_in_neither_stage_records_nothing() {
        let task = task_at(TaskState::Reviewing, 7);
        let task_id = task.id();
        let (mut repository, mut time, saved) = handles(Ok(Some(task)));

        let outcome = super::record_uncertain_background(&mut repository, &mut time, task_id);

        assert_eq!(outcome, super::BackgroundRecoveryOutcome::NotApplicable);
        assert!(saved.lock().expect("saved transitions mutex").is_empty());
    }
}
