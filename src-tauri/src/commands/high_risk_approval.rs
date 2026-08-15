//! Explicit high-risk category approval IPC (Unit 5b-4).
//!
//! This module only records and reports a user's own explicit choice of one
//! of the 13 fixed `HighRiskCategory` values. There is no Policy Engine
//! here: nothing in this module classifies, infers, recommends, or expands
//! which category applies to which operation, and nothing in it starts a
//! provider, drives `AutoFixing`/`ReviewFixing`/`Merging`, or blocks any
//! execution. Both handlers call `TaskService`'s already-dormant use cases
//! (`get_high_risk_approval_status`/`approve_high_risk_operation`) directly
//! and convert the result to a content-free DTO — no background thread, no
//! run registry, no adapter, and no state transition. `TaskService`'s own
//! preconditions already leave task state, version, transition history, and
//! the `ActiveTaskLease` untouched; these handlers add nothing on top.

use chatoms_application::tasks::{ApproveHighRiskOperationRequest, TaskService};
use chatoms_ports::TimeProvider;

use crate::{
    dto::{HighRiskApprovalDto, HighRiskApprovalStatusDto, HighRiskCategoryDto},
    error::IpcErrorDto,
    state::ManagedRuntime,
};

use super::tasks::parse_task_id;

/// Read-only: whether an exact `(task_id, expected_version, risk_category)`
/// approval already exists. Never creates, reuses, or mutates an approval,
/// and never touches task state, version, transition history, or the
/// `ActiveTaskLease`. A stale version or a genuine repository failure is
/// propagated as an error by `TaskService::get_high_risk_approval_status`,
/// never converted to `approved: false`.
pub fn handle_get_high_risk_approval_status(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
    risk_category: HighRiskCategoryDto,
) -> Result<HighRiskApprovalStatusDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .get_high_risk_approval_status(id, expected_version, risk_category.into())
        .map(HighRiskApprovalStatusDto::from)
        .map_err(IpcErrorDto::from)
}

/// Atomically creates-or-reuses the exact `(task_id, expected_version,
/// risk_category)` approval the caller explicitly selected. Delegates
/// entirely to `TaskService::approve_high_risk_operation`
/// (`FoundationRepository::ensure_high_risk_approval`, Unit 5b-3), which
/// never distinguishes "just created" from "already existed" in its
/// result shape. Starts no provider, spawns no background thread, and
/// never changes task state, version, transition history, or the
/// `ActiveTaskLease`.
pub fn handle_approve_high_risk_operation(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
    risk_category: HighRiskCategoryDto,
) -> Result<HighRiskApprovalDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let mut ready = runtime.ready_snapshot()?;
    let approved_at_ms = ready.time.now_ms().map_err(|_| IpcErrorDto::internal())?;
    TaskService::new(&mut ready.repository, &mut ready.time)
        .approve_high_risk_operation(ApproveHighRiskOperationRequest::new(
            id,
            expected_version,
            risk_category.into(),
            approved_at_ms,
        ))
        .map(HighRiskApprovalDto::from)
        .map_err(IpcErrorDto::from)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_high_risk_approval_status(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
    risk_category: HighRiskCategoryDto,
) -> Result<HighRiskApprovalStatusDto, IpcErrorDto> {
    handle_get_high_risk_approval_status(&state, &task_id, expected_version, risk_category)
}

#[tauri::command(rename_all = "camelCase")]
pub fn approve_high_risk_operation(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
    risk_category: HighRiskCategoryDto,
) -> Result<HighRiskApprovalDto, IpcErrorDto> {
    handle_approve_high_risk_operation(&state, &task_id, expected_version, risk_category)
}
