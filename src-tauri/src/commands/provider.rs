use std::path::{Path, PathBuf};

use chatoms_application::provider::ProviderConfigService;
use chatoms_application::system::CapabilityStatus as AppCapabilityStatus;
use chatoms_infrastructure::process::StdProcessRunner;
use chatoms_infrastructure::provider::StdProviderCapabilityAdapter;
use chatoms_ports::provider::ProviderCapabilityPort;

use crate::{
    dto::{
        CapabilityStatusDto, RefreshClaudeCapabilityDto, RefreshOutcomeDto,
        SetClaudeExecutablePathDto,
    },
    error::IpcErrorDto,
    state::{CachedProviderCapabilities, ManagedRuntime, ProviderCapabilityHandle, RefreshOutcome},
};

fn cached_to_dto(status: Option<AppCapabilityStatus>) -> CapabilityStatusDto {
    match status {
        Some(AppCapabilityStatus::Supported) => CapabilityStatusDto::Supported,
        Some(AppCapabilityStatus::Unsupported) => CapabilityStatusDto::Unsupported,
        None => CapabilityStatusDto::Unavailable,
    }
}

pub fn handle_set_claude_executable_path(
    runtime: &ManagedRuntime,
    path: &str,
) -> Result<SetClaudeExecutablePathDto, IpcErrorDto> {
    let ready = runtime.ready_snapshot()?;
    let display = mask_executable_path(Path::new(path));
    {
        let mut repository = ready.repository.clone();
        let mut time = ready.time.clone();
        let mut service = ProviderConfigService::new(&mut repository, &mut time);
        service
            .set_claude_executable_path(path)
            .map_err(IpcErrorDto::from)?;
    }
    ready.provider_capabilities.invalidate_and_bump_generation();
    Ok(SetClaudeExecutablePathDto {
        display_path: display,
        claude_execution: CapabilityStatusDto::Unavailable,
    })
}

pub fn handle_refresh_claude_capability(
    runtime: &ManagedRuntime,
) -> Result<RefreshClaudeCapabilityDto, IpcErrorDto> {
    let ready = runtime.ready_snapshot()?;
    let handle = &ready.provider_capabilities;
    let captured_generation = match handle.try_begin_refresh() {
        Some(g) => g,
        None => {
            let cached = handle.read_cache();
            return Ok(RefreshClaudeCapabilityDto {
                outcome: RefreshOutcomeDto::Conflict,
                claude_execution: cached_to_dto(cached.claude),
                codex_execution: cached_to_dto(cached.codex),
            });
        }
    };

    let probe_result = run_probe(&ready.repository, &ready.time, &ready.preflight_dir, handle);

    match probe_result {
        Ok(capabilities) => {
            let outcome = handle.finish_refresh(captured_generation, capabilities);
            let cached = handle.read_cache();
            Ok(RefreshClaudeCapabilityDto {
                outcome: match outcome {
                    RefreshOutcome::Completed => RefreshOutcomeDto::Completed,
                    RefreshOutcome::Superseded => RefreshOutcomeDto::Superseded,
                    RefreshOutcome::Conflict => RefreshOutcomeDto::Conflict,
                },
                claude_execution: cached_to_dto(cached.claude),
                codex_execution: cached_to_dto(cached.codex),
            })
        }
        Err(error) => {
            handle.abort_refresh();
            Err(error)
        }
    }
}

fn run_probe(
    repository: &crate::state::RepositoryHandle,
    time: &crate::state::TimeProviderHandle,
    preflight_dir: &Option<crate::state::PreflightDirectory>,
    _handle: &ProviderCapabilityHandle,
) -> Result<CachedProviderCapabilities, IpcErrorDto> {
    let executable_path: Option<PathBuf> = {
        let mut repo = repository.clone();
        let mut t = time.clone();
        let mut service = ProviderConfigService::new(&mut repo, &mut t);
        service
            .get_claude_binding()
            .map_err(IpcErrorDto::from)?
            .and_then(|binding| binding.executable_path.map(PathBuf::from))
    };

    let mut adapter = StdProviderCapabilityAdapter::new(
        executable_path,
        preflight_dir.clone(),
        StdProcessRunner::new(),
    );
    let capabilities = adapter
        .provider_capabilities()
        .map_err(|_| IpcErrorDto::internal())?;

    Ok(CachedProviderCapabilities {
        claude: Some(port_to_app_capability(capabilities.claude)),
        codex: Some(port_to_app_capability(capabilities.codex)),
    })
}

fn port_to_app_capability(
    value: chatoms_ports::provider::ProviderCapabilityStatus,
) -> AppCapabilityStatus {
    match value {
        chatoms_ports::provider::ProviderCapabilityStatus::Supported => {
            AppCapabilityStatus::Supported
        }
        chatoms_ports::provider::ProviderCapabilityStatus::Unsupported => {
            AppCapabilityStatus::Unsupported
        }
    }
}

fn mask_executable_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            let profile_path = Path::new(&profile);
            if let (Ok(canonical_path), Ok(canonical_profile)) = (
                std::fs::canonicalize(path),
                std::fs::canonicalize(profile_path),
            ) && let Ok(suffix) = canonical_path.strip_prefix(&canonical_profile)
            {
                let display_suffix = suffix.to_string_lossy().replace('/', "\\");
                if display_suffix.is_empty() {
                    return "%USERPROFILE%".to_owned();
                }
                return format!("%USERPROFILE%\\{display_suffix}");
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    safe_filename(path)
}

fn safe_filename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("executable")
        .to_owned()
}

#[tauri::command]
pub fn set_claude_executable_path(
    state: tauri::State<'_, ManagedRuntime>,
    path: String,
) -> Result<SetClaudeExecutablePathDto, IpcErrorDto> {
    handle_set_claude_executable_path(&state, &path)
}

#[tauri::command]
pub fn refresh_claude_capability(
    state: tauri::State<'_, ManagedRuntime>,
) -> Result<RefreshClaudeCapabilityDto, IpcErrorDto> {
    handle_refresh_claude_capability(&state)
}
