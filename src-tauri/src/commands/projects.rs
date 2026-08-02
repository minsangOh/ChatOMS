use chatoms_application::projects::ProjectService;

use crate::{
    dto::ProjectDto,
    error::IpcErrorDto,
    state::{ManagedRuntime, RuntimeState},
};

pub fn handle_list_projects(runtime: &ManagedRuntime) -> Result<Vec<ProjectDto>, IpcErrorDto> {
    let mut state = runtime.lock()?;
    match &mut *state {
        RuntimeState::Ready(ready) => ProjectService::new(&mut ready.repository)
            .list_projects()
            .map(|projects| projects.into_iter().map(ProjectDto::from).collect())
            .map_err(IpcErrorDto::from),
        RuntimeState::Unavailable(unavailable) => Err(unavailable.error.clone().into()),
    }
}

#[tauri::command]
pub fn list_projects(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<Vec<ProjectDto>, IpcErrorDto> {
    handle_list_projects(&state)
}
