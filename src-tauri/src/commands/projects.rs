use std::str::FromStr;

use chatoms_application::{
    error::ApplicationError,
    projects::{ProjectMutationService, ProjectService, RegisterProjectRequest},
};
use chatoms_domain::ProjectId;

use crate::{
    dto::{ProjectCandidateDto, ProjectDto, ProjectStatusDto},
    error::IpcErrorDto,
    state::ManagedRuntime,
};

pub fn handle_list_projects(runtime: &ManagedRuntime) -> Result<Vec<ProjectDto>, IpcErrorDto> {
    let mut ready = runtime.ready_snapshot()?;
    ProjectService::new(&mut ready.repository)
        .list_projects()
        .map(|projects| projects.into_iter().map(ProjectDto::from).collect())
        .map_err(IpcErrorDto::from)
}

pub fn handle_inspect_project_candidate(
    runtime: &ManagedRuntime,
    input_path: &str,
) -> Result<ProjectCandidateDto, IpcErrorDto> {
    let mut ready = runtime.ready_snapshot()?;
    let mut service = ProjectMutationService::new(
        &mut ready.repository,
        &mut ready.git,
        &mut ready.filesystem,
        &mut ready.time,
    );
    service
        .inspect_candidate(input_path)
        .map(ProjectCandidateDto::from)
        .map_err(Into::into)
}

pub fn handle_register_project(
    runtime: &ManagedRuntime,
    input_path: String,
    confirmation_token: String,
    name: Option<String>,
) -> Result<ProjectDto, IpcErrorDto> {
    let mut ready = runtime.ready_snapshot()?;
    let mut service = ProjectMutationService::new(
        &mut ready.repository,
        &mut ready.git,
        &mut ready.filesystem,
        &mut ready.time,
    );
    service
        .register_project(RegisterProjectRequest {
            input_path,
            confirmation_token,
            name,
        })
        .map(ProjectDto::from)
        .map_err(Into::into)
}

pub fn handle_get_project_git_status(
    runtime: &ManagedRuntime,
    project_id: &str,
) -> Result<ProjectStatusDto, IpcErrorDto> {
    let project_id = ProjectId::from_str(project_id)
        .map_err(|error| IpcErrorDto::from(ApplicationError::from_domain(&error)))?;
    let mut ready = runtime.ready_snapshot()?;
    let mut service = ProjectMutationService::new(
        &mut ready.repository,
        &mut ready.git,
        &mut ready.filesystem,
        &mut ready.time,
    );
    service
        .project_status(project_id)
        .map(ProjectStatusDto::from)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_projects(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<Vec<ProjectDto>, IpcErrorDto> {
    handle_list_projects(&state)
}

#[tauri::command(rename_all = "camelCase")]
pub fn inspect_project_candidate(
    state: tauri::State<'_, ManagedRuntime>,
    input_path: String,
) -> Result<ProjectCandidateDto, IpcErrorDto> {
    handle_inspect_project_candidate(&state, &input_path)
}

#[tauri::command(rename_all = "camelCase")]
pub fn register_project(
    state: tauri::State<'_, ManagedRuntime>,
    input_path: String,
    confirmation_token: String,
    name: Option<String>,
) -> Result<ProjectDto, IpcErrorDto> {
    handle_register_project(&state, input_path, confirmation_token, name)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_project_git_status(
    state: tauri::State<'_, ManagedRuntime>,
    project_id: String,
) -> Result<ProjectStatusDto, IpcErrorDto> {
    handle_get_project_git_status(&state, &project_id)
}
