use chatoms_application::tasks::TaskService;
use chatoms_domain::TaskState;

use crate::{
    commands::tasks::parse_task_id, dto::PostMergeValidationResultDto, error::IpcErrorDto,
    state::ManagedRuntime,
};

pub fn handle_get_post_merge_validation_results(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Vec<PostMergeValidationResultDto>, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let mut service = TaskService::new(&mut ready.repository, &mut ready.time);
    let task = service
        .get_task(id)
        .map_err(IpcErrorDto::from)?
        .ok_or_else(IpcErrorDto::not_found)?;
    if !matches!(
        task.state,
        TaskState::Completed | TaskState::RecoveryRequired
    ) {
        return Ok(Vec::new());
    }
    service
        .get_post_merge_validation_results(id)
        .map(|results| {
            results
                .into_iter()
                .map(PostMergeValidationResultDto::from)
                .collect()
        })
        .map_err(IpcErrorDto::from)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_post_merge_validation_results(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Vec<PostMergeValidationResultDto>, IpcErrorDto> {
    handle_get_post_merge_validation_results(&state, &task_id)
}
