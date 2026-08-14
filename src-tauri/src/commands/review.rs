use std::path::PathBuf;

use chatoms_application::{
    error::ApplicationError,
    provider::ProviderConfigService,
    review_execution::{
        BeginReviewExecutionRequest, ReviewExecutionRecorder, ReviewExecutionStarter,
    },
    tasks::TaskService,
};
use chatoms_domain::{TaskId, TaskState};
use chatoms_infrastructure::{
    claude_review::ClaudeReviewAdapter, git::GitCliAdapter, process::StdProcessRunner,
    provider::StdProviderCapabilityAdapter, redaction::SecretRedactor,
};
use chatoms_ports::{TimeProvider, error::FailureCategory};

use crate::{
    dto::{CancelReviewDto, ReviewResultDto, TaskDto},
    error::IpcErrorDto,
    state::{AppRuntime, ManagedRuntime, ReviewRunRegistry},
};

use super::tasks::parse_task_id;

/// Unregisters `task_id` from `review_runs` when dropped — on normal
/// completion of the background thread's closure body, and (this is the
/// point of the guard) during unwinding if anything in that closure ever
/// panics despite
/// [`ReviewExecutionRecorder::run_and_record_with_panic_containment`]
/// already containing the one realistic panic source (the executor).
/// `Drop` runs on unwind regardless of what panicked or where, which is a
/// stronger guarantee than any code that only runs on the non-panicking
/// path — so `cancel_claude_review` can never observe a stale registry
/// entry for a thread that has actually finished, crashed or not. Mirrors
/// `commands::planning::UnregisterOnDrop`.
struct UnregisterOnDrop {
    review_runs: ReviewRunRegistry,
    task_id: TaskId,
}

impl Drop for UnregisterOnDrop {
    fn drop(&mut self) {
        self.review_runs.unregister(self.task_id);
    }
}

/// Starts a Claude Review attempt: fresh-checks Claude capability, resolves
/// the read-only Git evidence `ReviewExecutionStarter::begin` needs (a fresh
/// [`GitCliAdapter`] used both for worktree identity re-verification and for
/// the bounded ephemeral diff read), and — only once every read-only
/// precondition (task state/version, `WorktreeReady` isolation, `TaskBrief`,
/// a usable diff) is confirmed — records or reuses the same-version
/// Claude/Review consent synchronously (so the caller gets an immediate,
/// accurate `TaskDto`; `Reviewing` itself never transitions). The actual
/// provider process then runs, and its outcome is recorded, on a detached
/// background thread. The thread is necessary — not merely convenient —
/// because a concurrent `cancel_claude_review` call must be able to reach
/// the in-flight run's cancellation handle while this command's own
/// (possibly long) process run is still in progress. Mirrors
/// `commands::planning::handle_start_claude_planning`.
pub fn handle_start_claude_review(
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

    // A fresh, ephemeral GitCliAdapter (never the shared `GitServiceHandle`,
    // which only implements the narrower `GitService` trait): one clone
    // verifies worktree identity, the other reads the bounded ephemeral
    // diff, mirroring `ReviewDiffReader`'s own two-role split.
    let git_adapter = GitCliAdapter::from_environment()
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    let mut git_for_verify = git_adapter.clone();
    let mut diff_port = git_adapter;
    let mut filesystem = ready.filesystem.clone();

    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let inputs = ReviewExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut precheck_capability,
        &mut git_for_verify,
        &mut filesystem,
        &mut diff_port,
    )
    .begin(BeginReviewExecutionRequest::new(id, expected_version))
    .map_err(IpcErrorDto::from)?;

    let started_at_ms = time.now_ms().map_err(|_| IpcErrorDto::internal())?;
    let task_dto = TaskDto::from(inputs.task.clone());

    let Some(cancellation) = ready.review_runs.register(id) else {
        // Unlike Planning/Implementation, `ReviewExecutionStarter::begin`
        // commits no state transition of its own (only a same-version
        // consent — `Reviewing` stays `Reviewing`), so a registry conflict
        // here has no transition to undo. A Review consent may already be
        // recorded, but that alone is not a safety problem (see
        // `TaskService::start_review`'s own idempotent reuse), so this
        // returns a typed error and leaves the task's state exactly as it
        // was, rather than falling back to `RecoveryRequired`.
        return Err(registry_conflict_error());
    };
    let review_runs = ready.review_runs.clone();
    let expected_version = inputs.task.version;
    let worktree_path = inputs.worktree_path;
    let brief = inputs.brief;
    let diff_text = inputs.diff_text;
    let mut repository_for_thread = ready.repository.clone();
    let mut time_for_thread = ready.time.clone();

    std::thread::spawn(move || {
        let _unregister_guard = UnregisterOnDrop {
            review_runs,
            task_id: id,
        };
        let executor_capability = StdProviderCapabilityAdapter::new(
            Some(executable_path.clone()),
            preflight_dir_handle,
            StdProcessRunner::new(),
        );
        let mut adapter = ClaudeReviewAdapter::new(
            executor_capability,
            StdProcessRunner::new(),
            executable_path,
            preflight_dir_path,
            redactor,
        );
        let _ = ReviewExecutionRecorder::new(&mut repository_for_thread, &mut time_for_thread)
            .run_and_record_with_panic_containment(
                id,
                expected_version,
                &worktree_path,
                &brief,
                &diff_text,
                started_at_ms,
                &mut adapter,
                &cancellation,
            );
    });

    Ok(task_dto)
}

/// Requests cancellation of an in-flight Claude Review run for `task_id`.
/// Returns whether a matching run was found; it does not itself change task
/// state — only a subsequently *confirmed* process exit is ever recorded as
/// `Paused` with `resume_target_state = Reviewing` (an unconfirmed
/// cancellation attempt falls back to `RecoveryRequired` and keeps the
/// active lease). Mirrors `commands::planning::handle_cancel_claude_planning`.
pub fn handle_cancel_claude_review(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<CancelReviewDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let requested = ready.review_runs.request_cancellation(id);
    Ok(CancelReviewDto { requested })
}

/// Reads back the already-safe, immutable Claude Review result for
/// `task_id`, but only while the task is currently
/// `AwaitingUserDiffApproval` — the one state this Unit approves surfacing it
/// in. Any other state (task not found aside) returns `Ok(None)`, which is
/// indistinguishable from "no result recorded yet"; this is deliberate,
/// since neither case is an error the caller can act on differently, and it
/// keeps this read-only surface from ever depending on
/// `record_review_result`'s outcome-to-state mapping staying anything other
/// than an internal implementation detail. Mirrors
/// `commands::planning::handle_get_planning_result`.
pub fn handle_get_review_result(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Option<ReviewResultDto>, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let mut service = TaskService::new(&mut ready.repository, &mut ready.time);
    let task = service
        .get_task(id)
        .map_err(IpcErrorDto::from)?
        .ok_or_else(IpcErrorDto::not_found)?;
    if task.state != TaskState::AwaitingUserDiffApproval {
        return Ok(None);
    }
    service
        .get_review_result(id)
        .map(|result| result.map(ReviewResultDto::from))
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
pub fn start_claude_review(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskDto, IpcErrorDto> {
    handle_start_claude_review(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_claude_review(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<CancelReviewDto, IpcErrorDto> {
    handle_cancel_claude_review(&state, &task_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_review_result(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Option<ReviewResultDto>, IpcErrorDto> {
    handle_get_review_result(&state, &task_id)
}

#[cfg(test)]
mod tests {
    use super::UnregisterOnDrop;
    use crate::state::ReviewRunRegistry;
    use chatoms_domain::TaskId;

    #[test]
    fn dropping_the_guard_normally_unregisters_the_entry() {
        let registry = ReviewRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");

        drop(UnregisterOnDrop {
            review_runs: registry.clone(),
            task_id,
        });

        assert!(
            !registry.request_cancellation(task_id),
            "a normal drop must have unregistered the entry"
        );
    }

    #[test]
    fn the_guard_unregisters_the_entry_even_when_a_panic_unwinds_through_it() {
        let registry = ReviewRunRegistry::new();
        let task_id = TaskId::new();
        registry.register(task_id).expect("first registration");
        let registry_for_guard = registry.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = UnregisterOnDrop {
                review_runs: registry_for_guard,
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
