use std::path::PathBuf;

use chatoms_application::{
    context_package_implementation_execution::{
        BeginContextPackageImplementationExecutionRequest,
        ContextPackageImplementationExecutionRecorder,
        ContextPackageImplementationExecutionStarter,
    },
    error::ApplicationError,
    implementation_execution::{
        BeginImplementationExecutionRequest, ImplementationExecutionRecorder,
        ImplementationExecutionStarter,
    },
    provider::ProviderConfigService,
    tasks::{RecordImplementationResultRequest, TaskService},
};
use chatoms_domain::TaskId;
use chatoms_infrastructure::{
    claude_implementation::ClaudeImplementationAdapter, process::StdProcessRunner,
    provider::StdProviderCapabilityAdapter, redaction::SecretRedactor,
};
use chatoms_ports::{
    TimeProvider, error::FailureCategory, repository::ImplementationResultOutcome,
};

use crate::{
    dto::{CancelImplementationDto, TaskDto},
    error::IpcErrorDto,
    state::{AppRuntime, ImplementationRunRegistry, ManagedRuntime},
};

use super::tasks::parse_task_id;

/// Unregisters `task_id` from `implementation_runs` when dropped — on
/// normal completion of the background thread's closure body, and (this is
/// the point of the guard) during unwinding if anything in that closure
/// ever panics despite
/// [`ImplementationExecutionRecorder::run_and_record_with_panic_containment`]
/// already containing the one realistic panic source (the executor).
/// `Drop` runs on unwind regardless of what panicked or where, which is a
/// stronger guarantee than any code that only runs on the non-panicking
/// path — so `cancel_claude_implementation` can never observe a stale
/// registry entry for a thread that has actually finished, crashed or not.
/// Mirrors `commands::planning::UnregisterOnDrop`.
struct UnregisterOnDrop {
    implementation_runs: ImplementationRunRegistry,
    task_id: TaskId,
}

impl Drop for UnregisterOnDrop {
    fn drop(&mut self) {
        self.implementation_runs.unregister(self.task_id);
    }
}

/// Starts a Claude Implementation attempt: fresh-checks capability and
/// commits the `AwaitingDesignApproval -> Implementing` transition
/// synchronously (so the caller gets an immediate, accurate `TaskDto`), then
/// runs the actual provider process and records its outcome on a detached
/// background thread. The thread is necessary — not merely convenient —
/// because a concurrent `cancel_claude_implementation` call must be able to
/// reach the in-flight run's cancellation handle while this command's own
/// (possibly long) process run is still in progress. Mirrors
/// `commands::planning::handle_start_claude_planning`.
pub fn handle_start_claude_implementation(
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
        ImplementationExecutionStarter::new(&mut repository, &mut time, &mut precheck_capability)
            .begin(BeginImplementationExecutionRequest::new(
                id,
                expected_version,
            ))
            .map_err(IpcErrorDto::from)?;

    let started_at_ms = time.now_ms().map_err(|_| IpcErrorDto::internal())?;
    let task_dto = TaskDto::from(inputs.task.clone());

    let Some(cancellation) = ready.implementation_runs.register(id) else {
        // The transition above already committed, so this is no longer a
        // "nothing written" rejection. A live registry entry for this task
        // id should be impossible here (the version-conflict guard in
        // `TaskService::start_implementation` already fails closed before a
        // second Implementation attempt could reach this point), so treat
        // it the same as any other post-transition invariant break: fall
        // back to `RecoveryRequired` rather than silently running without a
        // cancellation handle.
        let mut repository = ready.repository.clone();
        let mut time = ready.time.clone();
        let started_at_ms = time.now_ms().unwrap_or(started_at_ms);
        let _ = TaskService::new(&mut repository, &mut time).record_implementation_result(
            RecordImplementationResultRequest::new(
                id,
                inputs.task.version,
                ImplementationResultOutcome::RecoveryRequired,
                None,
                None,
                started_at_ms,
                "application".to_owned(),
                "task.implementation.registry-conflict".to_owned(),
            ),
        );
        return Err(registry_conflict_error());
    };
    let implementation_runs = ready.implementation_runs.clone();
    let expected_version = inputs.task.version;
    let worktree_path = inputs.worktree_path;
    let brief = inputs.brief;
    let plan_text = inputs.plan_text;
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();

    std::thread::spawn(move || {
        let _unregister_guard = UnregisterOnDrop {
            implementation_runs,
            task_id: id,
        };
        let executor_capability = StdProviderCapabilityAdapter::new(
            Some(executable_path.clone()),
            preflight_dir_handle,
            StdProcessRunner::new(),
        );
        let mut adapter = ClaudeImplementationAdapter::new(
            executor_capability,
            StdProcessRunner::new(),
            executable_path,
            preflight_dir_path,
            redactor,
        );
        let _ =
            ImplementationExecutionRecorder::new(&mut repository_for_thread, &mut time_for_thread)
                .run_and_record_with_panic_containment(
                    id,
                    expected_version,
                    &worktree_path,
                    &brief,
                    &plan_text,
                    started_at_ms,
                    &mut adapter,
                    &cancellation,
                );
    });

    Ok(task_dto)
}

/// Starts a Context Package v1 Claude Implementation activation:
/// fresh-checks capability, then delegates to
/// [`ContextPackageImplementationExecutionStarter::begin`], which loads and
/// validates all required evidence (isolation, stored Claude Planning
/// result, `TaskBrief`) *before* committing the
/// `AwaitingDesignApproval -> Implementing` transition — unlike
/// [`handle_start_claude_planning_context_package`]'s "commit first, fall
/// back after" shape, a successful `begin()` call here means the transition
/// and the evidence fetch have already both happened, so there is no
/// separate post-commit evidence-fetch failure to fall back from. Runs the
/// actual provider process and records its outcome on the same kind of
/// detached background thread, sharing the exact same
/// [`ImplementationRunRegistry`] [`handle_start_claude_implementation`]
/// uses — a task can only ever have one `Implementing` attempt in flight
/// regardless of which path started it, so no second registry or cancel
/// command is needed; [`handle_cancel_claude_implementation`] and
/// [`crate::commands::tasks`]'s startup reconciliation already work
/// unchanged for this path.
pub fn handle_start_claude_implementation_context_package(
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
    let inputs = ContextPackageImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut precheck_capability,
    )
    .begin(BeginContextPackageImplementationExecutionRequest::new(
        id,
        expected_version,
    ))
    .map_err(IpcErrorDto::from)?;

    let started_at_ms = time.now_ms().map_err(|_| IpcErrorDto::internal())?;
    let task_dto = TaskDto::from(inputs.task.clone());

    let Some(cancellation) = ready.implementation_runs.register(id) else {
        // Same reasoning as `handle_start_claude_implementation`: the
        // transition above already committed, so this is no longer a
        // "nothing written" rejection, and a live registry entry for this
        // task id should be impossible here (the version-conflict guard in
        // `TaskService::start_context_package_implementation` already fails
        // closed before a second Implementation attempt could reach this
        // point).
        let mut repository = ready.repository.clone();
        let mut time = ready.time.clone();
        let started_at_ms = time.now_ms().unwrap_or(started_at_ms);
        let _ = TaskService::new(&mut repository, &mut time).record_implementation_result(
            RecordImplementationResultRequest::new(
                id,
                inputs.task.version,
                ImplementationResultOutcome::RecoveryRequired,
                None,
                None,
                started_at_ms,
                "application".to_owned(),
                "task.implementation.context_package.registry-conflict".to_owned(),
            ),
        );
        return Err(registry_conflict_error());
    };
    let implementation_runs = ready.implementation_runs.clone();
    let expected_version = inputs.task.version;
    let worktree_path = inputs.worktree_path;
    let brief = inputs.brief;
    let plan_text = inputs.plan_text;
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();

    std::thread::spawn(move || {
        let _unregister_guard = UnregisterOnDrop {
            implementation_runs,
            task_id: id,
        };
        let executor_capability = StdProviderCapabilityAdapter::new(
            Some(executable_path.clone()),
            preflight_dir_handle,
            StdProcessRunner::new(),
        );
        let mut adapter = ClaudeImplementationAdapter::new(
            executor_capability,
            StdProcessRunner::new(),
            executable_path,
            preflight_dir_path,
            redactor,
        );
        let _ = ContextPackageImplementationExecutionRecorder::new(
            &mut repository_for_thread,
            &mut time_for_thread,
        )
        .run_and_record_with_panic_containment(
            id,
            expected_version,
            &worktree_path,
            &brief,
            &plan_text,
            started_at_ms,
            &mut adapter,
            &cancellation,
        );
    });

    Ok(task_dto)
}

/// Requests cancellation of an in-flight Claude Implementation run for
/// `task_id`. Returns whether a matching run was found; it does not itself
/// change task state — only a subsequently *confirmed* process exit is ever
/// recorded as `Paused` (an unconfirmed cancellation attempt falls back to
/// `RecoveryRequired` and keeps the active lease). Mirrors
/// `commands::planning::handle_cancel_claude_planning`.
pub fn handle_cancel_claude_implementation(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<CancelImplementationDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let requested = ready.implementation_runs.request_cancellation(id);
    Ok(CancelImplementationDto { requested })
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
pub fn start_claude_implementation(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    handle_start_claude_implementation(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn start_claude_implementation_context_package(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    handle_start_claude_implementation_context_package(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_claude_implementation(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<CancelImplementationDto, IpcErrorDto> {
    handle_cancel_claude_implementation(&state, &task_id)
}

#[cfg(test)]
mod tests {
    use super::UnregisterOnDrop;
    use crate::state::ImplementationRunRegistry;
    use chatoms_domain::TaskId;

    #[test]
    fn dropping_the_guard_normally_unregisters_the_entry() {
        let registry = ImplementationRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");

        drop(UnregisterOnDrop {
            implementation_runs: registry.clone(),
            task_id,
        });

        assert!(
            !registry.request_cancellation(task_id),
            "a normal drop must have unregistered the entry"
        );
    }

    #[test]
    fn the_guard_unregisters_the_entry_even_when_a_panic_unwinds_through_it() {
        let registry = ImplementationRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");
        let registry_for_guard = registry.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = UnregisterOnDrop {
                implementation_runs: registry_for_guard,
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
