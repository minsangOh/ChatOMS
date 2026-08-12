use chatoms_application::system::SystemService;
use chatoms_ports::git::GitService;

use crate::{
    dto::{
        BootstrapStatusDto, HealthDto, HealthStateDto, LegacyMigrationDiagnosticDto,
        SystemStatusDto, VersionDto,
    },
    error::IpcErrorDto,
    state::{ManagedRuntime, RuntimeSnapshot},
};

pub fn handle_get_version(runtime: &ManagedRuntime) -> Result<VersionDto, IpcErrorDto> {
    let version = match runtime.snapshot()? {
        RuntimeSnapshot::Ready(mut ready) => {
            SystemService::new(&ready.bootstrap_status, &mut ready.capabilities).get_version()
        }
        RuntimeSnapshot::Unavailable {
            bootstrap_status, ..
        } => bootstrap_status
            .as_ref()
            .map_or(chatoms_application::APPLICATION_VERSION, |status| {
                status.application_version
            }),
    };
    Ok(VersionDto { version })
}

pub fn handle_get_health(runtime: &ManagedRuntime) -> Result<HealthDto, IpcErrorDto> {
    match runtime.snapshot()? {
        RuntimeSnapshot::Ready(mut ready) => {
            let mut service = SystemService::new(&ready.bootstrap_status, &mut ready.capabilities);
            service
                .get_health()
                .map(|status| HealthDto {
                    status: status.into(),
                })
                .map_err(IpcErrorDto::from)
        }
        RuntimeSnapshot::Unavailable { .. } => Ok(HealthDto {
            status: HealthStateDto::Unavailable,
        }),
    }
}

pub fn handle_get_system_status(runtime: &ManagedRuntime) -> Result<SystemStatusDto, IpcErrorDto> {
    match runtime.snapshot()? {
        RuntimeSnapshot::Ready(mut ready) => {
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
            let cached = ready.provider_capabilities.read_cache();
            status.capabilities.claude_execution = cached_to_dto(cached.claude);
            status.capabilities.codex_execution = cached_to_dto(cached.codex);
            Ok(status)
        }
        RuntimeSnapshot::Unavailable { error, .. } => Err(error.into()),
    }
}

fn cached_to_dto(
    status: Option<chatoms_application::system::CapabilityStatus>,
) -> crate::dto::CapabilityStatusDto {
    match status {
        Some(chatoms_application::system::CapabilityStatus::Supported) => {
            crate::dto::CapabilityStatusDto::Supported
        }
        Some(chatoms_application::system::CapabilityStatus::Unsupported) => {
            crate::dto::CapabilityStatusDto::Unsupported
        }
        None => crate::dto::CapabilityStatusDto::Unavailable,
    }
}

pub fn handle_get_bootstrap_status(
    runtime: &ManagedRuntime,
) -> Result<BootstrapStatusDto, IpcErrorDto> {
    match runtime.snapshot()? {
        RuntimeSnapshot::Ready(ready) => Ok(ready.bootstrap_status.into()),
        RuntimeSnapshot::Unavailable {
            error,
            bootstrap_status,
            ..
        } => bootstrap_status
            .clone()
            .map(BootstrapStatusDto::from)
            .ok_or_else(|| error.into()),
    }
}

pub fn handle_get_legacy_migration_diagnostic(
    runtime: &ManagedRuntime,
) -> Result<Option<LegacyMigrationDiagnosticDto>, IpcErrorDto> {
    Ok(match runtime.snapshot()? {
        RuntimeSnapshot::Ready(_) => None,
        RuntimeSnapshot::Unavailable {
            migration_diagnostic,
            ..
        } => migration_diagnostic.map(LegacyMigrationDiagnosticDto::from),
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
