use chatoms_application::{
    error::ApplicationError,
    testing_execution::{BeginTestingBatchRequest, TestingBatchRecorder, TestingBatchStarter},
};
use chatoms_domain::TaskId;
use chatoms_infrastructure::{
    process::StdProcessRunner, validation_execution::CargoValidationAdapter,
};
use chatoms_ports::error::FailureCategory;

use crate::{
    dto::{CancelTestingDto, TaskDto},
    error::IpcErrorDto,
    state::{ManagedRuntime, TestingRunRegistry},
};

use super::tasks::parse_task_id;

/// Unregisters `task_id` from `testing_runs` when dropped — on normal
/// completion of the background thread's closure body, and (this is the
/// point of the guard) during unwinding if anything in that closure ever
/// panics despite
/// [`TestingBatchRecorder::run_and_record_with_panic_containment`] already
/// containing the one realistic panic source (the executor, contained
/// per-command rather than around the whole batch — see that method's own
/// docs). `Drop` runs on unwind regardless of what panicked or where, which
/// is a stronger guarantee than any code that only runs on the
/// non-panicking path — so `cancel_validation_testing` can never observe a
/// stale registry entry for a thread that has actually finished, crashed or
/// not. Mirrors `commands::planning::UnregisterOnDrop`.
struct UnregisterOnDrop {
    testing_runs: TestingRunRegistry,
    task_id: TaskId,
}

impl Drop for UnregisterOnDrop {
    fn drop(&mut self) {
        self.testing_runs.unregister(self.task_id);
    }
}

/// Starts a Cargo-only Testing batch: read-only verifies the task is
/// `Testing` at `expected_version`, resolves its worktree, and loads every
/// approved validation command for the current version — committing no
/// state transition of its own (the task is already `Testing`; only the
/// eventual batch outcome moves it to `Reviewing`/`Paused`/
/// `RecoveryRequired`) — then runs the approved commands and records the
/// batch's outcome on a detached background thread. The thread is
/// necessary — not merely convenient — because a concurrent
/// `cancel_validation_testing` call must be able to reach the in-flight
/// run's cancellation handle while this command's own (possibly long)
/// batch is still in progress. Mirrors
/// `commands::planning::handle_start_claude_planning`.
pub fn handle_start_validation_testing(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;

    let Some(app_temp_dir) = ready.app_temp_dir() else {
        return Err(unsupported_capability_error());
    };

    let mut repository = ready.repository.clone();
    let mut filesystem_for_start = ready.filesystem.clone();
    let inputs = TestingBatchStarter::new(&mut repository, &mut filesystem_for_start)
        .begin(BeginTestingBatchRequest::new(id, expected_version))
        .map_err(IpcErrorDto::from)?;

    let task_dto = TaskDto::from(inputs.task.clone());

    let Some(cancellation) = ready.testing_runs.register(id) else {
        // Unlike Planning/Implementation, TestingBatchStarter::begin commits
        // no state transition (the task is already Testing), so a registry
        // conflict here has nothing to undo: return a typed error and leave
        // the task exactly as it was, rather than falling back to
        // RecoveryRequired.
        return Err(registry_conflict_error());
    };
    let testing_runs = ready.testing_runs.clone();
    let worktree_identity = inputs.worktree_identity;
    let approvals = inputs.approvals;
    let filesystem = ready.filesystem.clone();
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();

    std::thread::spawn(move || {
        let _unregister_guard = UnregisterOnDrop {
            testing_runs,
            task_id: id,
        };
        let mut adapter =
            CargoValidationAdapter::new(StdProcessRunner::new(), filesystem, app_temp_dir);
        let _ = TestingBatchRecorder::new(&mut repository_for_thread, &mut time_for_thread)
            .run_and_record_with_panic_containment(
                id,
                expected_version,
                &worktree_identity,
                &approvals,
                &mut adapter,
                &cancellation,
            );
    });

    Ok(task_dto)
}

/// Requests cancellation of an in-flight Testing batch for `task_id`.
/// Returns whether a matching run was found; it does not itself change task
/// state — only a subsequently *confirmed* cancelled command is ever
/// recorded (as `Paused` with `resume_target_state = Testing`, via
/// `TestingBatchRecorder`/`TaskService::finalize_validation_command_batch`);
/// an unconfirmed cancellation attempt falls back to `RecoveryRequired` and
/// keeps the active lease. Mirrors
/// `commands::planning::handle_cancel_claude_planning`.
pub fn handle_cancel_validation_testing(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<CancelTestingDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let requested = ready.testing_runs.request_cancellation(id);
    Ok(CancelTestingDto { requested })
}

fn unsupported_capability_error() -> IpcErrorDto {
    ApplicationError::from_failure(
        FailureCategory::Unsupported,
        FailureCategory::Unsupported.default_severity(),
        FailureCategory::Unsupported.default_retry(),
    )
    .into()
}

fn registry_conflict_error() -> IpcErrorDto {
    ApplicationError::from_failure(
        FailureCategory::InvariantViolation,
        FailureCategory::InvariantViolation.default_severity(),
        FailureCategory::InvariantViolation.default_retry(),
    )
    .into()
}

#[tauri::command(rename_all = "camelCase")]
pub fn start_validation_testing(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    handle_start_validation_testing(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_validation_testing(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<CancelTestingDto, IpcErrorDto> {
    handle_cancel_validation_testing(&state, &task_id)
}

#[cfg(test)]
mod tests {
    use super::UnregisterOnDrop;
    use crate::state::TestingRunRegistry;
    use chatoms_domain::TaskId;

    #[test]
    fn dropping_the_guard_normally_unregisters_the_entry() {
        let registry = TestingRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");

        drop(UnregisterOnDrop {
            testing_runs: registry.clone(),
            task_id,
        });

        assert!(
            !registry.request_cancellation(task_id),
            "a normal drop must have unregistered the entry"
        );
    }

    #[test]
    fn the_guard_unregisters_the_entry_even_when_a_panic_unwinds_through_it() {
        let registry = TestingRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");
        let registry_for_guard = registry.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = UnregisterOnDrop {
                testing_runs: registry_for_guard,
                task_id,
            };
            panic!("simulated panic while the guard is still alive");
        }));

        assert!(result.is_err(), "the simulated panic must actually occur");
        assert!(
            !registry.request_cancellation(task_id),
            "the guard's Drop must unregister the entry during unwinding, not only on normal exit"
        );
    }
}
