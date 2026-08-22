//! Read-only observation of the process-local
//! [`crate::state::MergeConflictWriteLock`].
//!
//! The UI cannot decide on its own when a merge-conflict Git write has
//! finished: a background `git merge --continue` or `git merge --abort`
//! leaves the task in `MergeConflict` for its whole duration, so "the task
//! is still `MergeConflict`" says nothing about whether a write is in
//! flight. A local in-flight flag that a polling tick clears is therefore
//! wrong by construction — it re-enables the actions while the write is
//! still running.
//!
//! This command exposes the authoritative answer instead. It is purely an
//! observation: no task mutation, no approval record, no Git process, no
//! registry change, and no change to the lock's `register`/`unregister`
//! semantics. It is a UX gate, not a safety boundary — the actual
//! protection against a concurrent second write remains the lock itself,
//! which both write commands acquire before doing anything.

use chatoms_domain::TaskId;

use crate::{
    dto::MergeConflictWriteStatusDto, error::IpcErrorDto, state::ManagedRuntime,
    state::MergeConflictWriteLock,
};

use super::tasks::parse_task_id;

pub fn handle_get_merge_conflict_write_status(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<MergeConflictWriteStatusDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    Ok(status_for(&ready.merge_conflict_writes, id))
}

/// Split out so the mapping from lock to DTO is testable without a
/// `ManagedRuntime`, and so it is obvious that nothing else happens here.
fn status_for(
    merge_conflict_writes: &MergeConflictWriteLock,
    task_id: TaskId,
) -> MergeConflictWriteStatusDto {
    MergeConflictWriteStatusDto {
        running: merge_conflict_writes.is_running(task_id),
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_merge_conflict_write_status(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<MergeConflictWriteStatusDto, IpcErrorDto> {
    handle_get_merge_conflict_write_status(&state, &task_id)
}

#[cfg(test)]
mod tests {
    use super::status_for;
    use crate::state::MergeConflictWriteLock;
    use chatoms_domain::TaskId;

    #[test]
    fn the_status_tracks_register_and_unregister_exactly() {
        let lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();

        assert!(!status_for(&lock, task_id).running);
        assert!(lock.register(task_id));
        assert!(status_for(&lock, task_id).running);
        lock.unregister(task_id);
        assert!(!status_for(&lock, task_id).running);
    }

    #[test]
    fn the_status_is_per_task_and_observing_it_never_acquires_the_lock() {
        let lock = MergeConflictWriteLock::new();
        let held = TaskId::new();
        let other = TaskId::new();
        assert!(lock.register(held));

        assert!(status_for(&lock, held).running);
        assert!(!status_for(&lock, other).running);
        // Repeated observation must not have registered anything: `other`
        // is still free to start a write, and `held` is still held.
        assert!(!status_for(&lock, other).running);
        assert!(
            lock.register(other),
            "observing a task's status must never take the lock for it"
        );
        assert!(
            !lock.register(held),
            "the genuinely held lock must still be held after being observed"
        );
    }

    #[test]
    fn every_clone_of_the_handle_observes_the_same_shared_state() {
        let lock = MergeConflictWriteLock::new();
        let clone = lock.clone();
        let task_id = TaskId::new();

        assert!(lock.register(task_id));
        assert!(
            status_for(&clone, task_id).running,
            "a clone shares the runtime's single lock, not a copy of it"
        );
        clone.unregister(task_id);
        assert!(!status_for(&lock, task_id).running);
    }
}
