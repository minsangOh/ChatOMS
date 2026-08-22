//! Tauri commands for `ContextPackageV1` consent/manifest preparation
//! (Unit 5a-4's dormant `TaskService::prepare_planning_context_package`/
//! `prepare_implementation_context_package`/`prepare_review_context_package`).
//!
//! Every handler here does exactly one thing: call the matching
//! `TaskService` preparation method synchronously and convert its result to
//! a content-free DTO. There is no background thread, no run registry, no
//! cancellation signal, no adapter, and no `StreamingProcessRunner` — unlike
//! `commands::planning`/`commands::implementation`/`commands::review`, which
//! all spawn a detached thread to run the real provider process, nothing in
//! this module ever starts a Claude/Codex process. `TaskService`'s own
//! preconditions (task state, version, and — for Planning — a
//! `WorktreeReady` isolation record) already leave task state, version,
//! transition history, and the `ActiveTaskLease` untouched; these handlers
//! add nothing on top of that and never call `save_transition`,
//! `terminate_task`, or any lease-affecting method.
//!
//! `start_claude_planning`/`start_claude_implementation`/`start_claude_review`
//! (the real, `LegacyPhase4`-scoped execution commands) are not modified by
//! this module and are not called from it.

use chatoms_application::tasks::{
    PrepareImplementationContextPackageRequest, PreparePlanningContextPackageRequest,
    PrepareReviewContextPackageRequest, TaskService,
};

use crate::{
    dto::{
        ContextPackageImplementationReadinessDto, ContextPackagePlanningReadinessDto,
        ContextPackagePreparationDto, ContextPackageReviewReadinessDto,
    },
    error::IpcErrorDto,
    state::ManagedRuntime,
};

use super::tasks::parse_task_id;

/// Reads back, read-only, whether an exact `(task_id, Claude, Planning,
/// expected_version, ContextPackageV1)` consent and its FK-bound manifest
/// already exist — see `TaskService::get_context_package_planning_readiness`.
/// Never creates, reuses, or mutates a consent/manifest, and never touches
/// task state, version, transition history, or the `ActiveTaskLease`.
pub fn handle_get_context_package_planning_readiness(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<ContextPackagePlanningReadinessDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .get_context_package_planning_readiness(id, expected_version)
        .map(ContextPackagePlanningReadinessDto::from)
        .map_err(IpcErrorDto::from)
}

/// Reads back, read-only, whether an exact `(task_id, Claude,
/// Implementation, expected_version, ContextPackageV1)` consent and its
/// FK-bound manifest already exist — see
/// `TaskService::get_context_package_implementation_readiness`. Never
/// creates, reuses, or mutates a consent/manifest, and never touches task
/// state, version, transition history, or the `ActiveTaskLease`. Says
/// nothing about whether a completed stored Claude Planning result exists —
/// that structural precondition is checked only when actually starting
/// Implementation.
pub fn handle_get_context_package_implementation_readiness(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<ContextPackageImplementationReadinessDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .get_context_package_implementation_readiness(id, expected_version)
        .map(ContextPackageImplementationReadinessDto::from)
        .map_err(IpcErrorDto::from)
}

/// Reads back, read-only, whether an exact `(task_id, Claude, Review,
/// expected_version, ContextPackageV1)` consent and its FK-bound manifest
/// already exist — see `TaskService::get_context_package_review_readiness`.
/// Never creates, reuses, or mutates a consent/manifest, and never touches
/// task state, version, transition history, or the `ActiveTaskLease`.
pub fn handle_get_context_package_review_readiness(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<ContextPackageReviewReadinessDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .get_context_package_review_readiness(id, expected_version)
        .map(ContextPackageReviewReadinessDto::from)
        .map_err(IpcErrorDto::from)
}

/// Prepares (creates or reuses) the exact `ContextPackageV1` consent and
/// manifest for a future Claude Planning attempt. Requires `WorktreeReady`
/// task state and a `WorktreeReady` isolation record — see
/// `TaskService::prepare_planning_context_package` for the full contract.
/// Never starts Claude Planning and never changes task state or version.
pub fn handle_prepare_planning_context_package(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<ContextPackagePreparationDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .prepare_planning_context_package(PreparePlanningContextPackageRequest::new(
            id,
            expected_version,
        ))
        .map(ContextPackagePreparationDto::from)
        .map_err(IpcErrorDto::from)
}

/// See [`handle_prepare_planning_context_package`]; requires
/// `AwaitingDesignApproval` task state instead. Never starts Claude
/// Implementation and never changes task state or version.
pub fn handle_prepare_implementation_context_package(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<ContextPackagePreparationDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .prepare_implementation_context_package(PrepareImplementationContextPackageRequest::new(
            id,
            expected_version,
        ))
        .map(ContextPackagePreparationDto::from)
        .map_err(IpcErrorDto::from)
}

/// See [`handle_prepare_planning_context_package`]; requires `Reviewing`
/// task state instead. Never starts Claude Review and never changes task
/// state or version.
pub fn handle_prepare_review_context_package(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<ContextPackagePreparationDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .prepare_review_context_package(PrepareReviewContextPackageRequest::new(
            id,
            expected_version,
        ))
        .map(ContextPackagePreparationDto::from)
        .map_err(IpcErrorDto::from)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_context_package_planning_readiness(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<ContextPackagePlanningReadinessDto, IpcErrorDto> {
    handle_get_context_package_planning_readiness(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_context_package_implementation_readiness(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<ContextPackageImplementationReadinessDto, IpcErrorDto> {
    handle_get_context_package_implementation_readiness(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_context_package_review_readiness(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<ContextPackageReviewReadinessDto, IpcErrorDto> {
    handle_get_context_package_review_readiness(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn prepare_planning_context_package(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<ContextPackagePreparationDto, IpcErrorDto> {
    handle_prepare_planning_context_package(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn prepare_implementation_context_package(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<ContextPackagePreparationDto, IpcErrorDto> {
    handle_prepare_implementation_context_package(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn prepare_review_context_package(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<ContextPackagePreparationDto, IpcErrorDto> {
    handle_prepare_review_context_package(&state, &task_id, expected_version)
}
