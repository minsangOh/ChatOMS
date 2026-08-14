//! Testing validation command discovery, approval-status, and approval IPC.
//!
//! Discovery and approval status are read-only. Approval never executes
//! anything and never changes `Task` state — it only pins one Cargo
//! candidate down together with a user-approved executable binding (see
//! `chatoms_application::validation_commands`). The frontend never supplies
//! or influences `executable`/`arguments`: every selected kind is resolved
//! against a fresh `ValidationCommandDiscovery` pass run here, then handed to
//! `ValidationCommandService::approve_command`, which re-validates the exact
//! same candidate match independently.

use std::collections::HashSet;
use std::path::PathBuf;

use chatoms_application::{
    error::ApplicationError,
    tasks::TaskService,
    validation_commands::{ApproveValidationCommandRequest, ValidationCommandService},
};
use chatoms_domain::{TaskId, ValidationCommandKind};
use chatoms_infrastructure::validation_discovery::ManifestValidationCommandDiscovery;
use chatoms_ports::{error::FailureCategory, repository::FoundationRepository};

use crate::{
    dto::{
        ApproveValidationCommandInputDto, ApproveValidationCommandResultDto,
        ValidationCommandApprovalStatusDto, ValidationCommandCandidateDto,
    },
    error::IpcErrorDto,
    state::ManagedRuntime,
};

use super::tasks::parse_task_id;

/// Read-only: the Cargo-only candidates `ValidationCommandDiscovery`
/// proposes for the task's current version right now. Filters out any
/// package-manager (`npm`/`pnpm`/`yarn`) candidate the shared discovery
/// service may also propose — this Unit surfaces Cargo commands only.
pub fn handle_get_validation_command_candidates(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<Vec<ValidationCommandCandidateDto>, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let version = current_task_version(&ready, id)?;

    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let mut discovery = ManifestValidationCommandDiscovery::new();
    let mut filesystem = ready.filesystem.clone();
    let candidates =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .list_candidates(id, version)
            .map_err(IpcErrorDto::from)?;

    Ok(candidates
        .iter()
        .filter(|candidate| candidate.executable == "cargo")
        .map(ValidationCommandCandidateDto::from_cargo_candidate)
        .collect())
}

/// Read-only: which `ValidationCommandKind`s already have an approved
/// binding for the task's current version.
pub fn handle_get_validation_command_approval_status(
    runtime: &ManagedRuntime,
    task_id: &str,
) -> Result<ValidationCommandApprovalStatusDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;
    let version = current_task_version(&ready, id)?;

    let mut repository = ready.repository.clone();
    let approvals = repository
        .list_validation_command_approvals(id, version)
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    Ok(ValidationCommandApprovalStatusDto {
        approved_kinds: approvals
            .into_iter()
            .map(|approval| approval.kind.into())
            .collect(),
    })
}

/// Approves every selected kind for `(task_id, expected_version)`. Rejects
/// an empty selection, a duplicate kind, or a blank executable path at the
/// boundary without touching storage. For each selected kind, re-derives
/// `executable`/`arguments` from a single fresh discovery pass (never from
/// the request) and rejects the whole request if any selected kind has no
/// matching Cargo candidate right now — nothing is approved in that case.
/// Optional `CARGO_HOME`/`RUSTUP_HOME` paths are passed through only when
/// non-blank; a blank value becomes `None`, never a guessed default.
pub fn handle_approve_validation_command(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
    input: ApproveValidationCommandInputDto,
) -> Result<ApproveValidationCommandResultDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    validate_approve_input(&input)?;

    let ready = runtime.ready_snapshot()?;
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let mut discovery = ManifestValidationCommandDiscovery::new();
    let mut filesystem = ready.filesystem.clone();

    let selected_kinds: Vec<ValidationCommandKind> = input
        .kinds
        .iter()
        .copied()
        .map(ValidationCommandKind::from)
        .collect();

    let candidates =
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .list_candidates(id, expected_version)
            .map_err(IpcErrorDto::from)?;

    let mut resolved = Vec::with_capacity(selected_kinds.len());
    for kind in selected_kinds {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.executable == "cargo" && candidate.kind == kind)
            .ok_or_else(invalid_input_error)?;
        resolved.push((
            kind,
            candidate.executable.clone(),
            candidate.arguments.clone(),
        ));
    }

    let executable_path = PathBuf::from(input.executable_path.trim());
    let cargo_home_path = normalize_optional_path(input.cargo_home_path.as_deref());
    let rustup_home_path = normalize_optional_path(input.rustup_home_path.as_deref());

    for (kind, executable, arguments) in resolved {
        ValidationCommandService::new(&mut repository, &mut time, &mut discovery, &mut filesystem)
            .approve_command(ApproveValidationCommandRequest::new(
                id,
                expected_version,
                kind,
                executable,
                arguments,
                executable_path.clone(),
                cargo_home_path.clone(),
                rustup_home_path.clone(),
            ))
            .map_err(IpcErrorDto::from)?;
    }

    let approvals = repository
        .list_validation_command_approvals(id, expected_version)
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    Ok(ApproveValidationCommandResultDto {
        approved_kinds: approvals
            .into_iter()
            .map(|approval| approval.kind.into())
            .collect(),
    })
}

fn current_task_version(
    ready: &crate::state::AppRuntime,
    task_id: TaskId,
) -> Result<u64, IpcErrorDto> {
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();
    let task = TaskService::new(&mut repository, &mut time)
        .get_task(task_id)
        .map_err(IpcErrorDto::from)?
        .ok_or_else(IpcErrorDto::not_found)?;
    Ok(task.version)
}

fn normalize_optional_path(value: Option<&str>) -> Option<PathBuf> {
    let trimmed = value.map(str::trim).filter(|value| !value.is_empty())?;
    Some(PathBuf::from(trimmed))
}

fn validate_approve_input(input: &ApproveValidationCommandInputDto) -> Result<(), IpcErrorDto> {
    if input.kinds.is_empty() {
        return Err(invalid_input_error());
    }
    let mut seen = HashSet::new();
    for kind in &input.kinds {
        if !seen.insert(*kind) {
            return Err(invalid_input_error());
        }
    }
    if input.executable_path.trim().is_empty() {
        return Err(invalid_input_error());
    }
    Ok(())
}

fn invalid_input_error() -> IpcErrorDto {
    ApplicationError::from_failure(
        FailureCategory::InvalidInput,
        FailureCategory::InvalidInput.default_severity(),
        FailureCategory::InvalidInput.default_retry(),
    )
    .into()
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_validation_command_candidates(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<Vec<ValidationCommandCandidateDto>, IpcErrorDto> {
    handle_get_validation_command_candidates(&state, &task_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_validation_command_approval_status(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
) -> Result<ValidationCommandApprovalStatusDto, IpcErrorDto> {
    handle_get_validation_command_approval_status(&state, &task_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn approve_validation_command(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
    input: ApproveValidationCommandInputDto,
) -> Result<ApproveValidationCommandResultDto, IpcErrorDto> {
    handle_approve_validation_command(&state, &task_id, expected_version, input)
}
