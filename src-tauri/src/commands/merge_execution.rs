use chatoms_application::{
    error::ApplicationError,
    merge_execution::{BeginMergeExecutionRequest, MergeExecutionRecorder, MergeExecutionStarter},
    tasks::{RecordMergeResultRequest, TaskService},
    user_diff_approval::{ApproveUserDiffRequest, UserDiffApprovalService},
};
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::{
    diff::DiffContentHash, error::FailureCategory, merge_execution::MergeExecutionOutcome,
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

    std::thread::spawn(move || {
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
            if result.is_err() {
                record_uncertain_merge(
                    &mut repository_for_thread,
                    &mut time_for_thread,
                    id,
                    expected_version,
                );
            }
        }));
        if result.is_err() {
            record_uncertain_merge(
                &mut repository_for_thread,
                &mut time_for_thread,
                id,
                expected_version,
            );
        }
    });

    Ok(task_dto)
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
