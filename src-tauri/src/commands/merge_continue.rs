use chatoms_application::{
    error::ApplicationError,
    manual_merge_resolution::{
        ConfirmManualMergeResolutionRequest, ManualMergeResolutionConfirmationService,
    },
    merge_continue::{BeginMergeContinueRequest, MergeContinueRecorder, MergeContinueStarter},
    tasks::{RecordMergeContinueResultRequest, TaskService},
};
use chatoms_domain::{TaskId, TaskState};
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::{
    error::FailureCategory, merge_continue::MergeContinueOutcome, repository::FoundationRepository,
};

use crate::{
    dto::TaskDto,
    error::IpcErrorDto,
    state::{ManagedRuntime, MergeConflictWriteLock},
};

use super::{
    merge_execution::{BackgroundRecoveryOutcome, record_post_merge_recovery},
    tasks::parse_task_id,
};

const ACTOR_KIND: &str = "user";
const RESULT_REASON: &str = "task.merge-continue.result";

/// Releases this task's [`MergeConflictWriteLock`] when dropped. Created
/// immediately after the lock is acquired and before anything else happens,
/// so every subsequent early return in the synchronous part of
/// `handle_confirm_manual_resolution_and_start_merge_continue` (a failed
/// Git runtime discovery, a rejected manual-resolution confirmation, a
/// rejected `MergeConflict -> Merging` start) releases the lock on the way
/// out. On the success path the guard is moved into the background thread,
/// where its `Drop` releases the lock when that thread finishes -- whether
/// the merge-continue write succeeded, returned an uncertain outcome, or
/// unwound from a panic. The lock is therefore never left held as a
/// recovery marker.
struct ReleaseWriteLockOnDrop {
    merge_conflict_writes: MergeConflictWriteLock,
    task_id: TaskId,
}

impl Drop for ReleaseWriteLockOnDrop {
    fn drop(&mut self) {
        self.merge_conflict_writes.unregister(self.task_id);
    }
}

pub fn handle_confirm_manual_resolution_and_start_merge_continue(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let mut filesystem = ready.filesystem.clone();

    // Cheap, Git-free fail-fast: an obviously wrong id/version/state is
    // rejected before this command ever discovers the trusted Git runtime.
    // `ManualMergeResolutionConfirmationService::confirm`/`MergeContinueStarter::begin`
    // still re-verify state/version (and every other precondition)
    // transactionally below — this check can only reject early, never
    // approve early, so it narrows rather than replaces that verification.
    let task = repository
        .get_task(id)
        .map_err(|error| IpcErrorDto::from(ApplicationError::from_categorized(&error)))?
        .ok_or_else(|| category_error(FailureCategory::NotFound))?;
    if task.version() != expected_version {
        return Err(category_error(FailureCategory::VersionConflict));
    }
    if task.state() != TaskState::MergeConflict {
        return Err(category_error(FailureCategory::InvalidState));
    }

    // Mutual exclusion with `commands::merge_abort` over this task's single
    // original checkout. Acquired before the trusted Git runtime is
    // discovered, before any confirmation row is recorded, before
    // `MergeConflict -> Merging` is committed, and before the background
    // thread is spawned -- so a rejection here has written nothing at all.
    if !ready.merge_conflict_writes.register(id) {
        return Err(category_error(FailureCategory::Conflict));
    }
    // From here on every exit path releases the lock: the early returns
    // below drop this guard, and the success path moves it into the
    // background thread.
    let write_lock_guard = ReleaseWriteLockOnDrop {
        merge_conflict_writes: ready.merge_conflict_writes.clone(),
        task_id: id,
    };

    let mut candidate = GitCliAdapter::from_environment()
        .map_err(|error| ApplicationError::from_categorized(&error))?;

    ManualMergeResolutionConfirmationService::new(
        &mut repository,
        &mut time,
        &mut filesystem,
        &mut candidate,
    )
    .confirm(ConfirmManualMergeResolutionRequest::new(
        id,
        expected_version,
    ))
    .map_err(IpcErrorDto::from)?;

    let inputs =
        MergeContinueStarter::new(&mut repository, &mut time, &mut filesystem, &mut candidate)
            .begin(BeginMergeContinueRequest::new(id, expected_version))
            .map_err(IpcErrorDto::from)?;

    let task_dto = TaskDto::from(inputs.task.clone());
    let merge_request = inputs.request;
    let started_version = inputs.task.version;
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();
    let mut filesystem_for_thread = ready.filesystem.clone();
    let app_temp_dir = ready.app_temp_dir();

    // `Builder::spawn` rather than `thread::spawn`: the latter panics when
    // the OS refuses a new thread. `write_lock_guard` has been moved into
    // the closure by that point, and on the `Err` path `spawn` owns and
    // drops the closure — so the guard's `Drop` releases the lock exactly as
    // it does on every other exit path, and the same task can immediately
    // try again. The `Merging` transition is already committed here, so the
    // failure path below also records the existing fail-closed recovery.
    let spawn_result = std::thread::Builder::new().spawn(move || {
        // Declared first so it is dropped last: the lock stays held for the
        // whole background write, including the fail-closed recovery
        // recording below, and is released on every exit from this closure.
        let _write_lock_guard = write_lock_guard;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut adapter = match GitCliAdapter::from_environment() {
                Ok(adapter) => adapter,
                Err(_) => {
                    record_uncertain_merge_continue(
                        &mut repository_for_thread,
                        &mut time_for_thread,
                        id,
                        started_version,
                    );
                    return;
                }
            };
            let result =
                MergeContinueRecorder::new(&mut repository_for_thread, &mut time_for_thread)
                    .run_and_record_with_panic_containment(
                        id,
                        started_version,
                        &merge_request,
                        &mut adapter,
                    );
            match result {
                Ok(view) if view.state == TaskState::PostMergeTesting => {
                    super::merge_execution::run_post_merge_validation(
                        &mut repository_for_thread,
                        &mut time_for_thread,
                        &mut filesystem_for_thread,
                        app_temp_dir,
                        id,
                        view.version,
                    );
                }
                Ok(_) => {}
                Err(_) => record_uncertain_merge_continue(
                    &mut repository_for_thread,
                    &mut time_for_thread,
                    id,
                    started_version,
                ),
            }
        }));
        if result.is_err() {
            let _ =
                record_uncertain_background(&mut repository_for_thread, &mut time_for_thread, id);
        }
    });
    if spawn_result.is_err() {
        // The lock was released when `spawn` dropped the closure. No Git
        // write happened, but `MergeContinueStarter::begin` has already
        // committed `MergeConflict -> Merging`, so the task is moved to the
        // existing fail-closed recovery outcome against its currently
        // persisted state and version rather than being left as if a
        // merge-continue were running. The raw spawn error is dropped here.
        let mut repository_for_recovery = ready.repository.clone();
        let mut time_for_recovery = ready.time.clone();
        let _ =
            record_uncertain_background(&mut repository_for_recovery, &mut time_for_recovery, id);
        return Err(category_error(FailureCategory::Internal));
    }

    Ok(task_dto)
}

fn record_uncertain_merge_continue(
    repository: &mut crate::state::RepositoryHandle,
    time: &mut crate::state::TimeProviderHandle,
    task_id: chatoms_domain::TaskId,
    expected_version: u64,
) {
    let _ = TaskService::new(repository, time).record_merge_continue_result(
        RecordMergeContinueResultRequest::new(
            task_id,
            expected_version,
            MergeContinueOutcome::PostWriteUncertain,
            ACTOR_KIND.to_owned(),
            RESULT_REASON.to_owned(),
        ),
    );
}

/// Mirrors [`crate::commands::merge_execution::record_uncertain_background`],
/// including why the stage *and* the version are both read from the task as
/// it is persisted now: a panic caught by the outer `catch_unwind` above
/// could have happened either inside the merge-continue write itself (task
/// still `Merging`) or inside the reused post-merge validation path that
/// follows a successful `Continued` (task already `PostMergeTesting`), and
/// the version this run started with is stale in the latter case.
fn record_uncertain_background(
    repository: &mut crate::state::RepositoryHandle,
    time: &mut crate::state::TimeProviderHandle,
    task_id: chatoms_domain::TaskId,
) -> BackgroundRecoveryOutcome {
    let Ok(Some(task)) = repository.get_task(task_id) else {
        return BackgroundRecoveryOutcome::Unresolved;
    };
    match task.state() {
        TaskState::Merging => {
            record_uncertain_merge_continue(repository, time, task_id, task.version());
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
    IpcErrorDto::from(ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    ))
}

#[tauri::command(rename_all = "camelCase")]
pub fn confirm_manual_resolution_and_start_merge_continue(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    handle_confirm_manual_resolution_and_start_merge_continue(&state, &task_id, expected_version)
}

#[cfg(test)]
mod tests {
    use super::super::merge_execution::tests::{handles, task_at};
    use super::{BackgroundRecoveryOutcome, ReleaseWriteLockOnDrop};
    use crate::state::MergeConflictWriteLock;
    use chatoms_domain::{TaskId, TaskState};
    use chatoms_ports::repository::RepositoryErrorCode;

    /// The same defect as
    /// `commands::merge_execution::tests::post_merge_recovery_uses_the_currently_persisted_version_not_the_starting_one`:
    /// a merge-continue background run starts at the `MergeConflict ->
    /// Merging` version, but once `Merging -> PostMergeTesting` commits, a
    /// panic escaping the outer containment must record recovery against
    /// the new version. The old code reused the starting version, so the
    /// write failed `VersionConflict` and was silently discarded.
    #[test]
    fn post_merge_recovery_uses_the_currently_persisted_version_not_the_starting_one() {
        let started_version = 5;
        let task = task_at(TaskState::PostMergeTesting, started_version + 1);
        let task_id = task.id();
        let (mut repository, mut time, saved) = handles(Ok(Some(task)));

        let outcome = super::record_uncertain_background(&mut repository, &mut time, task_id);

        assert_eq!(outcome, BackgroundRecoveryOutcome::Recorded);
        assert_eq!(
            *saved.lock().expect("saved transitions mutex"),
            vec![(started_version + 1, TaskState::RecoveryRequired)]
        );
    }

    #[test]
    fn a_failed_task_lookup_is_reported_unresolved_rather_than_silently_succeeding() {
        let (mut repository, mut time, saved) = handles(Err(RepositoryErrorCode::OperationFailed));

        let outcome = super::record_uncertain_background(&mut repository, &mut time, TaskId::new());

        assert_eq!(outcome, BackgroundRecoveryOutcome::Unresolved);
        assert!(saved.lock().expect("saved transitions mutex").is_empty());
    }

    #[test]
    fn dropping_the_guard_when_the_background_thread_finishes_releases_the_lock() {
        let lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(lock.register(task_id));

        drop(ReleaseWriteLockOnDrop {
            merge_conflict_writes: lock.clone(),
            task_id,
        });

        assert!(
            lock.register(task_id),
            "a normal drop must release the lock so a later merge-conflict write can start"
        );
    }

    #[test]
    fn the_guard_releases_the_lock_even_when_a_panic_unwinds_through_it() {
        let lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(lock.register(task_id));
        let lock_for_guard = lock.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = ReleaseWriteLockOnDrop {
                merge_conflict_writes: lock_for_guard,
                task_id,
            };
            panic!("simulated panic while the background write still holds the lock");
        }));

        assert!(result.is_err(), "the simulated panic must actually occur");
        assert!(
            lock.register(task_id),
            "the guard's Drop must release the lock during unwinding, not only on normal exit"
        );
    }

    /// `Builder::spawn` takes ownership of the closure it is handed and, on
    /// the `Err` path the OS can produce under thread exhaustion, drops it
    /// without ever running it. The production code has already moved
    /// `write_lock_guard` into that closure by then, so this models exactly
    /// what a refused spawn does: the lock must come back, and the same task
    /// must be able to start a merge-conflict write again immediately.
    #[test]
    fn a_closure_owning_the_guard_releases_the_lock_when_it_is_dropped_unrun() {
        let lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(lock.register(task_id));
        let write_lock_guard = ReleaseWriteLockOnDrop {
            merge_conflict_writes: lock.clone(),
            task_id,
        };

        let never_run = move || {
            let _write_lock_guard = write_lock_guard;
        };
        drop(never_run);

        assert!(
            lock.register(task_id),
            "a spawn that never ran the closure must still have released the lock"
        );
    }

    #[test]
    fn the_guard_only_releases_its_own_task_id() {
        let lock = MergeConflictWriteLock::new();
        let released = TaskId::new();
        let other = TaskId::new();
        assert!(lock.register(released));
        assert!(lock.register(other));

        drop(ReleaseWriteLockOnDrop {
            merge_conflict_writes: lock.clone(),
            task_id: released,
        });

        assert!(
            lock.register(released),
            "the guard's own task id is released"
        );
        assert!(
            !lock.register(other),
            "another task's in-flight merge-conflict write must stay excluded"
        );
    }
}
