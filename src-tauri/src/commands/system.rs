use chatoms_application::system::SystemService;
use chatoms_ports::git::GitService;

use crate::{
    dto::{
        BootstrapStatusDto, HealthDto, HealthStateDto, LegacyMigrationDiagnosticDto,
        SystemStatusDto, VersionDto,
    },
    error::IpcErrorDto,
    state::{ManagedRuntime, RuntimeState},
};

pub fn handle_get_version(runtime: &ManagedRuntime) -> Result<VersionDto, IpcErrorDto> {
    let mut state = runtime.lock()?;
    let version = match &mut *state {
        RuntimeState::Ready(ready) => {
            SystemService::new(&ready.bootstrap_status, &mut ready.capabilities).get_version()
        }
        RuntimeState::Unavailable(unavailable) => unavailable
            .bootstrap_status
            .as_ref()
            .map_or(chatoms_application::APPLICATION_VERSION, |status| {
                status.application_version
            }),
    };
    Ok(VersionDto { version })
}

pub fn handle_get_health(runtime: &ManagedRuntime) -> Result<HealthDto, IpcErrorDto> {
    let mut state = runtime.lock()?;
    match &mut *state {
        RuntimeState::Ready(ready) => {
            let mut service = SystemService::new(&ready.bootstrap_status, &mut ready.capabilities);
            service
                .get_health()
                .map(|status| HealthDto {
                    status: status.into(),
                })
                .map_err(IpcErrorDto::from)
        }
        RuntimeState::Unavailable(_) => Ok(HealthDto {
            status: HealthStateDto::Unavailable,
        }),
    }
}

pub fn handle_get_system_status(runtime: &ManagedRuntime) -> Result<SystemStatusDto, IpcErrorDto> {
    let mut state = runtime.lock()?;
    match &mut *state {
        RuntimeState::Ready(ready) => {
            let mut service = SystemService::new(&ready.bootstrap_status, &mut ready.capabilities);
            let mut status = service
                .get_system_status()
                .map(SystemStatusDto::from)
                .map_err(IpcErrorDto::from)?;
            status.capabilities.git_execution = if ready.git.is_available().unwrap_or(false) {
                crate::dto::CapabilityStatusDto::Supported
            } else {
                crate::dto::CapabilityStatusDto::Unavailable
            };
            Ok(status)
        }
        RuntimeState::Unavailable(unavailable) => Err(unavailable.error.clone().into()),
    }
}

pub fn handle_get_bootstrap_status(
    runtime: &ManagedRuntime,
) -> Result<BootstrapStatusDto, IpcErrorDto> {
    let state = runtime.lock()?;
    match &*state {
        RuntimeState::Ready(ready) => Ok(ready.bootstrap_status.clone().into()),
        RuntimeState::Unavailable(unavailable) => unavailable
            .bootstrap_status
            .clone()
            .map(BootstrapStatusDto::from)
            .ok_or_else(|| unavailable.error.clone().into()),
    }
}

pub fn handle_get_legacy_migration_diagnostic(
    runtime: &ManagedRuntime,
) -> Result<Option<LegacyMigrationDiagnosticDto>, IpcErrorDto> {
    let state = runtime.lock()?;
    Ok(match &*state {
        RuntimeState::Ready(_) => None,
        RuntimeState::Unavailable(unavailable) => unavailable
            .migration_diagnostic
            .clone()
            .map(LegacyMigrationDiagnosticDto::from),
    })
}

#[tauri::command]
pub fn get_version(state: tauri::State<'_, ManagedRuntime>) -> Result<VersionDto, IpcErrorDto> {
    handle_get_version(&state)
}

#[tauri::command]
pub fn get_health(state: tauri::State<'_, ManagedRuntime>) -> Result<HealthDto, IpcErrorDto> {
    handle_get_health(&state)
}

#[tauri::command]
pub fn get_system_status(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<SystemStatusDto, IpcErrorDto> {
    handle_get_system_status(&state)
}

#[tauri::command]
pub fn get_bootstrap_status(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<BootstrapStatusDto, IpcErrorDto> {
    handle_get_bootstrap_status(&state)
}

#[tauri::command]
pub fn get_legacy_migration_diagnostic(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<Option<LegacyMigrationDiagnosticDto>, IpcErrorDto> {
    handle_get_legacy_migration_diagnostic(&state)
}
