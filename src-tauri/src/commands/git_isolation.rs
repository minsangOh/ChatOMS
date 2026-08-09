use std::str::FromStr;

use chatoms_application::{error::ApplicationError, git_isolation::GitIsolationService};
use chatoms_domain::{ProjectId, TaskId};

use crate::{
    dto::TaskIsolationDto,
    error::IpcErrorDto,
    state::{ManagedRuntime, RuntimeState},
};

fn project_id(value: &str) -> Result<ProjectId, IpcErrorDto> {
    ProjectId::from_str(value).map_err(|error| ApplicationError::from_domain(&error).into())
}

fn task_id(value: &str) -> Result<TaskId, IpcErrorDto> {
    TaskId::from_str(value).map_err(|error| ApplicationError::from_domain(&error).into())
}

fn with_service(
    runtime: &ManagedRuntime,
    operation: impl FnOnce(
        &mut GitIsolationService<
            '_,
            crate::state::RepositoryHandle,
            crate::state::GitServiceHandle,
            crate::state::FilesystemIdentityHandle,
            crate::state::WorktreePathHandle,
            crate::state::TimeProviderHandle,
        >,
    ) -> Result<
        chatoms_application::git_isolation::TaskIsolationView,
        ApplicationError,
    >,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    let mut state = runtime.lock()?;
    match &mut *state {
        RuntimeState::Ready(ready) => {
            let mut service = GitIsolationService::new(
                &mut ready.repository,
                &mut ready.git,
                &mut ready.filesystem,
                &mut ready.worktree_paths,
                &mut ready.time,
            );
            operation(&mut service)
                .map(TaskIsolationDto::from)
                .map_err(Into::into)
        }
        RuntimeState::Unavailable(unavailable) => Err(unavailable.error.clone().into()),
    }
}

pub fn handle_create_isolation_task(
    runtime: &ManagedRuntime,
    project_id_value: &str,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    let id = project_id(project_id_value)?;
    with_service(runtime, |service| service.create_task(id))
}

pub fn handle_get_task_isolation(
    runtime: &ManagedRuntime,
    task_id_value: &str,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    let id = task_id(task_id_value)?;
    with_service(runtime, |service| service.get_task_isolation(id))
}

pub fn handle_approve_git_initialization(
    runtime: &ManagedRuntime,
    task_id_value: &str,
    expected_version: u64,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    let id = task_id(task_id_value)?;
    with_service(runtime, |service| {
        service.approve_git_initialization(id, expected_version)
    })
}

pub fn handle_create_task_worktree(
    runtime: &ManagedRuntime,
    task_id_value: &str,
    expected_version: u64,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    let id = task_id(task_id_value)?;
    with_service(runtime, |service| {
        service.create_task_worktree(id, expected_version)
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_isolation_task(
    state: tauri::State<'_, ManagedRuntime>,
    project_id: String,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    handle_create_isolation_task(&state, &project_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_task_isolation(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    handle_get_task_isolation(&state, &task_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn approve_git_initialization(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    handle_approve_git_initialization(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_task_worktree(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    handle_create_task_worktree(&state, &task_id, expected_version)
}
