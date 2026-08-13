use chatoms_application::{
    provider_eligibility::ProviderEligibilityPolicy, system::ProviderCapabilitySummary,
    tasks::TaskService,
};

use crate::{dto::ProviderEligibilityDto, error::IpcErrorDto, state::ManagedRuntime};

pub fn handle_get_provider_eligibility(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Vec<ProviderEligibilityDto>, IpcErrorDto> {
    let task_id = super::tasks::parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let task = TaskService::new(&mut ready.repository, &mut ready.time)
        .get_task(task_id)
        .map_err(IpcErrorDto::from)?
        .ok_or_else(IpcErrorDto::not_found)?;
    let cached = ready.provider_capabilities.read_cache();
    let capabilities = ProviderCapabilitySummary {
        claude: cached.claude,
        codex: cached.codex,
    };
    Ok(
        ProviderEligibilityPolicy::evaluate(task.state, capabilities)
            .into_iter()
            .map(ProviderEligibilityDto::from)
            .collect(),
    )
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_provider_eligibility(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Vec<ProviderEligibilityDto>, IpcErrorDto> {
    handle_get_provider_eligibility(&state, &task_id)
}
