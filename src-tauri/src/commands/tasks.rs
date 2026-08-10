use std::str::FromStr;

use chatoms_application::{error::ApplicationError, tasks::TaskService};
use chatoms_domain::TaskId;

use crate::{
    dto::{ActiveTaskDto, TaskDto, TaskTransitionDto},
    error::IpcErrorDto,
    state::ManagedRuntime,
};

pub fn handle_get_active_task(
    runtime: &ManagedRuntime,
) -> Result<Option<ActiveTaskDto>, IpcErrorDto> {
    let mut ready = runtime.ready_snapshot()?;
    let mut service = TaskService::new(&mut ready.repository, &mut ready.time);
    service
        .get_active_task()
        .map(|task| task.map(ActiveTaskDto::from))
        .map_err(IpcErrorDto::from)
}

pub fn handle_get_task(runtime: &ManagedRuntime, task_id: &str) -> Result<TaskDto, IpcErrorDto> {
    let task_id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let mut service = TaskService::new(&mut ready.repository, &mut ready.time);
    service
        .get_task(task_id)
        .map_err(IpcErrorDto::from)?
        .map(TaskDto::from)
        .ok_or_else(IpcErrorDto::not_found)
}

pub fn handle_list_task_history(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Vec<TaskTransitionDto>, IpcErrorDto> {
    let task_id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let mut service = TaskService::new(&mut ready.repository, &mut ready.time);
    service
        .task_history(task_id)
        .map(|history| history.into_iter().map(TaskTransitionDto::from).collect())
        .map_err(IpcErrorDto::from)
}

fn parse_task_id(value: &str) -> Result<TaskId, IpcErrorDto> {
    TaskId::from_str(value)
        .map_err(|error| IpcErrorDto::from(ApplicationError::from_domain(&error)))
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_active_task(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<Option<ActiveTaskDto>, IpcErrorDto> {
    handle_get_active_task(&state)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_task(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<TaskDto, IpcErrorDto> {
    handle_get_task(&state, &task_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_task_history(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Vec<TaskTransitionDto>, IpcErrorDto> {
    handle_list_task_history(&state, &task_id)
}
