use std::path::PathBuf;

use chatoms_application::{
    error::ApplicationError,
    planning_execution::{
        BeginPlanningExecutionRequest, PlanningExecutionRecorder, PlanningExecutionStarter,
    },
    provider::ProviderConfigService,
    tasks::{RecordPlanningResultRequest, TaskService},
};
use chatoms_domain::{TaskId, TaskState};
use chatoms_infrastructure::{
    claude_planning::ClaudePlanningAdapter, process::StdProcessRunner,
    provider::StdProviderCapabilityAdapter, redaction::SecretRedactor,
};
use chatoms_ports::{TimeProvider, error::FailureCategory, repository::PlanningResultOutcome};

use crate::{
    dto::{CancelPlanningDto, PlanningResultDto, TaskDto},
    error::IpcErrorDto,
    state::{AppRuntime, ManagedRuntime, PlanningRunRegistry},
};

use super::tasks::parse_task_id;

/// Unregisters `task_id` from `planning_runs` when dropped — on normal
/// completion of the background thread's closure body, and (this is the
/// point of the guard) during unwinding if anything in that closure ever
/// panics despite [`PlanningExecutionRecorder::run_and_record_with_panic_containment`]
/// already containing the one realistic panic source (the executor).
/// `Drop` runs on unwind regardless of what panicked or where, which is a
/// stronger guarantee than any code that only runs on the non-panicking
/// path — so `cancel_claude_planning` can never observe a stale registry
/// entry for a thread that has actually finished, crashed or not.
struct UnregisterOnDrop {
    planning_runs: PlanningRunRegistry,
    task_id: TaskId,
}

impl Drop for UnregisterOnDrop {
    fn drop(&mut self) {
        self.planning_runs.unregister(self.task_id);
    }
}

/// Starts a Claude Planning attempt: fresh-checks capability and commits the
/// `WorktreeReady -> Planning` transition synchronously (so the caller gets
/// an immediate, accurate `TaskDto`), then runs the actual provider process
/// and records its outcome on a detached background thread. The thread is
/// necessary — not merely convenient — because a concurrent
/// `cancel_claude_planning` call must be able to reach the in-flight run's
/// cancellation handle while this command's own (possibly long) process run
/// is still in progress.
pub fn handle_start_claude_planning(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;

    let Some(executable_path) = claude_executable_path(&ready)? else {
        return Err(unsupported_capability_error());
    };
    let preflight_dir_handle = ready.preflight_dir.clone();
    let Some(preflight_dir_path) = preflight_dir_handle
        .as_ref()
        .map(|dir| dir.path().to_path_buf())
    else {
        return Err(unsupported_capability_error());
    };
    let redactor = SecretRedactor::new().map_err(|_| IpcErrorDto::internal())?;

    let mut precheck_capability = StdProviderCapabilityAdapter::new(
        Some(executable_path.clone()),
        preflight_dir_handle.clone(),
        StdProcessRunner::new(),
    );

    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let inputs =
        PlanningExecutionStarter::new(&mut repository, &mut time, &mut precheck_capability)
            .begin(BeginPlanningExecutionRequest::new(id, expected_version))
            .map_err(IpcErrorDto::from)?;

    let started_at_ms = time.now_ms().map_err(|_| IpcErrorDto::internal())?;
    let task_dto = TaskDto::from(inputs.task.clone());

    let Some(cancellation) = ready.planning_runs.register(id) else {
        // The transition above already committed, so this is no longer a
        // "nothing written" rejection. A live registry entry for this task
        // id should be impossible here (the version-conflict guard in
        // `TaskService::start_planning` already fails closed before a second
        // Planning attempt could reach this point), so treat it the same as
        // any other post-transition invariant break: fall back to
        // `RecoveryRequired` rather than silently running without a
        // cancellation handle.
        let mut repository = ready.repository.clone();
        let mut time = ready.time.clone();
        let started_at_ms = time.now_ms().unwrap_or(started_at_ms);
        let _ = TaskService::new(&mut repository, &mut time).record_planning_result(
            RecordPlanningResultRequest::new(
                id,
                inputs.task.version,
                PlanningResultOutcome::RecoveryRequired,
                None,
                None,
                None,
                started_at_ms,
                "application".to_owned(),
                "task.planning.registry-conflict".to_owned(),
            ),
        );
        return Err(registry_conflict_error());
    };
    let planning_runs = ready.planning_runs.clone();
    let expected_version = inputs.task.version;
    let worktree_path = inputs.worktree_path;
    let brief = inputs.brief;
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();

    std::thread::spawn(move || {
        let _unregister_guard = UnregisterOnDrop {
            planning_runs,
            task_id: id,
        };
        let executor_capability = StdProviderCapabilityAdapter::new(
            Some(executable_path.clone()),
            preflight_dir_handle,
            StdProcessRunner::new(),
        );
        let mut adapter = ClaudePlanningAdapter::new(
            executor_capability,
            StdProcessRunner::new(),
            executable_path,
            preflight_dir_path,
            redactor,
        );
        let _ = PlanningExecutionRecorder::new(&mut repository_for_thread, &mut time_for_thread)
            .run_and_record_with_panic_containment(
                id,
                expected_version,
                &worktree_path,
                &brief,
                started_at_ms,
                &mut adapter,
                &cancellation,
            );
    });

    Ok(task_dto)
}

/// Requests cancellation of an in-flight Claude Planning run for `task_id`.
/// Returns whether a matching run was found; it does not itself change task
/// state — only a subsequently *confirmed* process exit is ever recorded as
/// `Cancelled` (an unconfirmed cancellation attempt falls back to
/// `RecoveryRequired` and keeps the active lease).
pub fn handle_cancel_claude_planning(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<CancelPlanningDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let requested = ready.planning_runs.request_cancellation(id);
    Ok(CancelPlanningDto { requested })
}

/// Reads back the already-safe, immutable Claude Planning result for
/// `task_id`, but only while the task is currently `AwaitingDesignApproval`
/// — the one state this Unit approves surfacing it in. Any other state (task
/// not found aside) returns `Ok(None)`, which is indistinguishable from "no
/// result recorded yet"; this is deliberate, since neither case is an error
/// the caller can act on differently, and it keeps this read-only surface
/// from ever depending on `record_planning_result`'s outcome-to-state
/// mapping staying anything other than an internal implementation detail.
pub fn handle_get_planning_result(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Option<PlanningResultDto>, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let mut service = TaskService::new(&mut ready.repository, &mut ready.time);
    let task = service
        .get_task(id)
        .map_err(IpcErrorDto::from)?
        .ok_or_else(IpcErrorDto::not_found)?;
    if task.state != TaskState::AwaitingDesignApproval {
        return Ok(None);
    }
    service
        .get_planning_result(id)
        .map(|result| result.map(PlanningResultDto::from))
        .map_err(IpcErrorDto::from)
}

fn claude_executable_path(ready: &AppRuntime) -> Result<Option<PathBuf>, IpcErrorDto> {
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    Ok(service
        .get_claude_binding()
        .map_err(IpcErrorDto::from)?
        .and_then(|binding| binding.executable_path.map(PathBuf::from)))
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
pub fn start_claude_planning(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    handle_start_claude_planning(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_claude_planning(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<CancelPlanningDto, IpcErrorDto> {
    handle_cancel_claude_planning(&state, &task_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_planning_result(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Option<PlanningResultDto>, IpcErrorDto> {
    handle_get_planning_result(&state, &task_id)
}

#[cfg(test)]
mod tests {
    use super::UnregisterOnDrop;
    use crate::state::PlanningRunRegistry;
    use chatoms_domain::TaskId;

    #[test]
    fn dropping_the_guard_normally_unregisters_the_entry() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");

        drop(UnregisterOnDrop {
            planning_runs: registry.clone(),
            task_id,
        });

        assert!(
            !registry.request_cancellation(task_id),
            "a normal drop must have unregistered the entry"
        );
    }

    #[test]
    fn the_guard_unregisters_the_entry_even_when_a_panic_unwinds_through_it() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");
        let registry_for_guard = registry.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = UnregisterOnDrop {
                planning_runs: registry_for_guard,
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
