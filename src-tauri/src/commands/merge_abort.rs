//! Tauri orchestration for Phase 5e-4: starts a user-approved abort of a
//! task's in-progress `MergeConflict` merge in the background, using the
//! Phase 5e-3 approval/preflight/adapter/application contract unchanged.
//! The task's state stays `MergeConflict` for the whole duration of the
//! background write -- only a subsequently confirmed `Aborted`/
//! `ConfirmedNotInMerge` outcome ever commits `MergeConflict -> Cancelled`
//! (see `TaskService::record_merge_abort_result`). No cancellation is
//! offered for this write (a `git merge --abort` is never interrupted
//! mid-flight); [`crate::state::MergeAbortRunRegistry`] exists solely to
//! reject a second concurrent attempt for the same task, never to hand back
//! a cancellation handle.
//!
//! A started abort additionally holds the process-local
//! [`crate::state::MergeConflictWriteLock`], which excludes it against
//! `commands::merge_continue`'s `git merge --continue` write over the same
//! original checkout. The two are separate: the registry rejects a second
//! *abort*, the shared lock rejects any *other* merge-conflict write. Both
//! are acquired before any approval or Git access and both are released by
//! the same RAII guard once the background write finishes.

use chatoms_application::{
    error::ApplicationError,
    merge_abort::{ApproveMergeAbortRequest, MergeAbortApprovalService, MergeAbortRecorder},
};
use chatoms_domain::{TaskId, TaskState};
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::{
    error::FailureCategory, merge_abort::MergeAbortRequest, repository::FoundationRepository,
};

use crate::{
    dto::MergeAbortStartDto,
    error::IpcErrorDto,
    state::{AppRuntime, ManagedRuntime, MergeAbortRunRegistry, MergeConflictWriteLock},
};

use super::tasks::parse_task_id;

/// Releases both of this abort's in-memory holds -- its `merge_abort_runs`
/// entry and its `merge_conflict_writes` lock -- when dropped: on normal
/// completion of the background thread's closure body, and (this is the
/// point of the guard) during unwinding if anything in that closure ever
/// panics despite
/// [`MergeAbortRecorder::run_and_record_with_panic_containment`] already
/// containing the one realistic panic source (the executor). `Drop` runs on
/// unwind regardless of what panicked or where, so neither a subsequent
/// `confirm_merge_abort_and_start` nor a subsequent
/// `confirm_manual_resolution_and_start_merge_continue` call can ever
/// observe a stale hold for a thread that has actually finished, crashed or
/// not. This also means an uncertain abort outcome releases both: they mark
/// "a write is executing", never "this task needs recovery".
struct UnregisterOnDrop {
    merge_abort_runs: MergeAbortRunRegistry,
    merge_conflict_writes: MergeConflictWriteLock,
    task_id: TaskId,
}

impl Drop for UnregisterOnDrop {
    fn drop(&mut self) {
        self.merge_abort_runs.unregister(self.task_id);
        self.merge_conflict_writes.unregister(self.task_id);
    }
}

/// Registers `task_id` in `merge_abort_runs` first (so a concurrent second
/// *abort* for the same task fails closed as `{ started: false }` without
/// touching approval, Git, or any repository write), then acquires the
/// shared [`MergeConflictWriteLock`] (so an abort never runs concurrently
/// with a merge-continue write over the same original checkout, also
/// reported as `{ started: false }` and releasing the registry entry it
/// just took), and only then records (creates or reuses) an immutable
/// merge-abort approval, re-runs the same read-only preflight to assemble a
/// [`MergeAbortRequest`], and spawns a detached background thread that
/// performs the actual `git merge --abort` write and records its outcome.
/// Any failure before the background thread is spawned releases both holds
/// first, so a retry is never blocked by this call's own failure.
pub fn handle_confirm_merge_abort_and_start(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<MergeAbortStartDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let mut repository = ready.repository.clone();

    // Cheap, Git-free fail-fast: an obviously wrong id/version/state is
    // rejected before this command ever registers a run or discovers the
    // trusted Git runtime. `MergeAbortApprovalService::approve` (via
    // `verify_abort_preconditions`) still re-verifies state/version (and
    // every other precondition) below — this check can only reject early,
    // never approve early, so it narrows rather than replaces that
    // verification. Mirrors `commands::merge_continue`'s identical pattern.
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

    if !ready.merge_abort_runs.register(id) {
        // Another abort attempt for this task is already in flight. Not an
        // error: no approval, Git, or repository write has happened here,
        // and the caller must not start a second background execution.
        return Ok(MergeAbortStartDto { started: false });
    }

    if !ready.merge_conflict_writes.register(id) {
        // A merge-conflict write for this task -- a merge-continue, or an
        // abort whose registry entry has already been released -- is
        // already in flight against the same original checkout. Release the
        // abort-only entry taken just above so this rejection leaks
        // nothing, and report the same `{ started: false }` the duplicate
        // case uses: no approval, preflight, Git write, or background
        // thread has happened here either.
        ready.merge_abort_runs.unregister(id);
        return Ok(MergeAbortStartDto { started: false });
    }

    match start_locked(&ready, id, expected_version) {
        Ok(()) => Ok(MergeAbortStartDto { started: true }),
        Err(error) => {
            ready.merge_abort_runs.unregister(id);
            ready.merge_conflict_writes.unregister(id);
            Err(error)
        }
    }
}

/// Everything after both holds have been taken: recording the approval,
/// re-running the preflight to assemble the request, and spawning the
/// background thread. Called only once `merge_abort_runs.register` and
/// `merge_conflict_writes.register` have both succeeded; every error path
/// here releases both, in the caller.
fn start_locked(ready: &AppRuntime, id: TaskId, expected_version: u64) -> Result<(), IpcErrorDto> {
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let mut filesystem = ready.filesystem.clone();

    let mut git = GitCliAdapter::from_environment()
        .map_err(|error| IpcErrorDto::from(ApplicationError::from_categorized(&error)))?;

    let approval =
        MergeAbortApprovalService::new(&mut repository, &mut time, &mut filesystem, &mut git)
            .approve(ApproveMergeAbortRequest::new(id, expected_version))
            .map_err(IpcErrorDto::from)?;

    let preflight = chatoms_application::merge_abort::verify_abort_preconditions(
        &mut repository,
        &mut filesystem,
        id,
        expected_version,
    )
    .map_err(IpcErrorDto::from)?;

    let merge_abort_request = MergeAbortRequest {
        original_checkout: preflight.original_checkout,
        original_common_dir: preflight.original_common_dir,
        task_worktree: preflight.task_worktree,
        project_id: preflight.task.project_id(),
        task_id: id,
        merge_conflict_task_version: approval.merge_conflict_task_version,
        source_approval_task_version: approval.source_approval_task_version,
        base_branch: preflight.base_branch,
        task_branch: preflight.task.task_branch_identity().as_str().to_owned(),
        base_commit: approval.base_commit,
        task_commit: approval.task_commit,
        merge_head_commit: approval.merge_head_commit,
    };

    let merge_abort_runs = ready.merge_abort_runs.clone();
    let merge_conflict_writes = ready.merge_conflict_writes.clone();
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();

    std::thread::spawn(move || {
        let _unregister_guard = UnregisterOnDrop {
            merge_abort_runs,
            merge_conflict_writes,
            task_id: id,
        };
        let Ok(mut git) = GitCliAdapter::from_environment() else {
            // No fallback state exists for this edge (see
            // `TaskService::record_merge_abort_result`'s documentation):
            // the task simply remains `MergeConflict`, and the immutable
            // approval survives for a later retry.
            return;
        };
        let _ = MergeAbortRecorder::new(&mut repository_for_thread, &mut time_for_thread)
            .run_and_record_with_panic_containment(
                id,
                expected_version,
                &merge_abort_request,
                &mut git,
            );
    });

    Ok(())
}

fn category_error(category: FailureCategory) -> IpcErrorDto {
    IpcErrorDto::from(ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    ))
}

#[tauri::command(rename_all = "camelCase")]
pub fn confirm_merge_abort_and_start(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<MergeAbortStartDto, IpcErrorDto> {
    handle_confirm_merge_abort_and_start(&state, &task_id, expected_version)
}

#[cfg(test)]
mod tests {
    use super::UnregisterOnDrop;
    use crate::state::{MergeAbortRunRegistry, MergeConflictWriteLock};
    use chatoms_domain::TaskId;

    #[test]
    fn dropping_the_guard_normally_unregisters_both_holds() {
        let registry = MergeAbortRunRegistry::new();
        let write_lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(registry.register(task_id));
        assert!(write_lock.register(task_id));

        drop(UnregisterOnDrop {
            merge_abort_runs: registry.clone(),
            merge_conflict_writes: write_lock.clone(),
            task_id,
        });

        assert!(
            registry.register(task_id),
            "a normal drop must have unregistered the abort-only entry"
        );
        assert!(
            write_lock.register(task_id),
            "a normal drop must have released the shared merge-conflict write lock"
        );
    }

    #[test]
    fn the_guard_unregisters_both_holds_even_when_a_panic_unwinds_through_it() {
        let registry = MergeAbortRunRegistry::new();
        let write_lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(registry.register(task_id));
        assert!(write_lock.register(task_id));
        let registry_for_guard = registry.clone();
        let write_lock_for_guard = write_lock.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = UnregisterOnDrop {
                merge_abort_runs: registry_for_guard,
                merge_conflict_writes: write_lock_for_guard,
                task_id,
            };
            panic!("simulated panic while the guard is still alive");
        }));

        assert!(result.is_err(), "the simulated panic must actually occur");
        assert!(
            registry.register(task_id),
            "the guard's Drop must unregister the entry during unwinding, not only on normal exit"
        );
        assert!(
            write_lock.register(task_id),
            "the guard's Drop must release the shared write lock during unwinding too"
        );
    }
}
