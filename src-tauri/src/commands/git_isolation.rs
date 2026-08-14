use std::str::FromStr;

use chatoms_application::{error::ApplicationError, git_isolation::GitIsolationService};
use chatoms_domain::{ProjectId, TaskId};
use chatoms_ports::error::FailureCategory;

use crate::{
    dto::{TaskBriefInputDto, TaskIsolationDto},
    error::IpcErrorDto,
    state::ManagedRuntime,
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
    let mut ready = runtime.ready_snapshot()?;
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

fn validate_brief_input(brief: &TaskBriefInputDto) -> Result<(), IpcErrorDto> {
    if brief.requirements.trim().is_empty()
        || brief.completion_criteria.trim().is_empty()
        || brief.prohibited_scope.trim().is_empty()
    {
        return Err(ApplicationError::from_failure(
            FailureCategory::InvalidInput,
            FailureCategory::InvalidInput.default_severity(),
            FailureCategory::InvalidInput.default_retry(),
        )
        .into());
    }
    Ok(())
}

pub fn handle_create_isolation_task(
    runtime: &ManagedRuntime,
    project_id_value: &str,
    brief: TaskBriefInputDto,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    validate_brief_input(&brief)?;
    let id = project_id(project_id_value)?;
    with_service(runtime, |service| {
        service.create_task(id, Some(brief.into()))
    })
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
    brief: TaskBriefInputDto,
) -> Result<TaskIsolationDto, IpcErrorDto> {
    handle_create_isolation_task(&state, &project_id, brief)
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
