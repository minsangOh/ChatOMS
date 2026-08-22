use chatoms_application::merge_conflict_inspection::MergeConflictInspectionService;
use chatoms_domain::TaskState;
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::repository::FoundationRepository;

use crate::{dto::MergeConflictInspectionDto, error::IpcErrorDto, state::ManagedRuntime};

use super::tasks::parse_task_id;

pub fn handle_get_merge_conflict_inspection(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Option<MergeConflictInspectionDto>, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let Some(task) = ready.repository.get_task(id).map_err(|error| {
        IpcErrorDto::from(chatoms_application::error::ApplicationError::from_categorized(&error))
    })?
    else {
        return Ok(None);
    };
    if task.state() != TaskState::MergeConflict {
        return Ok(None);
    }
    let mut git = match GitCliAdapter::from_environment() {
        Ok(adapter) => adapter,
        Err(_) => return Ok(Some(MergeConflictInspectionDto::unavailable())),
    };
    let result =
        MergeConflictInspectionService::new(&mut ready.repository, &mut ready.filesystem, &mut git)
            .inspect(id)
            .map_err(IpcErrorDto::from)?;
    Ok(result.map(Into::into))
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_merge_conflict_inspection(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Option<MergeConflictInspectionDto>, IpcErrorDto> {
    handle_get_merge_conflict_inspection(&state, &task_id)
}
