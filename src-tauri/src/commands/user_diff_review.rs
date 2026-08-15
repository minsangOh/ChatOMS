//! Tauri commands for the scoped, local-user-only diff review exception
//! (see `docs/DECISIONS.md`): `get_user_diff_for_review` hands the task's
//! current worktree diff, once, directly to the requesting user's own
//! review modal — the only IPC surface in this codebase that ever returns
//! raw repository diff content. `approve_user_diff` records a
//! content-free, hash-bound approval; it never accepts or returns raw diff
//! text. Neither command touches task state, version, transition history,
//! or the `ActiveTaskLease`, starts any provider, or gates/starts Merging —
//! both are read-only or approval-only, synchronous, and use no background
//! thread or run registry.

use chatoms_application::{
    error::ApplicationError,
    user_diff_approval::{
        ApproveUserDiffRequest, ReadUserDiffForReviewRequest, UserDiffApprovalService,
        UserDiffReviewReader,
    },
};
use chatoms_infrastructure::git::GitCliAdapter;
use chatoms_ports::{diff::DiffContentHash, error::FailureCategory};

use crate::{
    dto::{RawUserDiffForReviewDto, UserDiffApprovalDto},
    error::IpcErrorDto,
    state::ManagedRuntime,
};

use super::tasks::parse_task_id;

/// Reads the task's current worktree diff read-only and returns it, once,
/// together with its content-free SHA-256 digest. Only succeeds while the
/// task is `AwaitingUserDiffApproval` at `expected_version` with a
/// `WorktreeReady` isolation record whose Git/filesystem identity
/// re-verifies fresh; any other outcome (state/version mismatch, identity
/// mismatch, no changes, oversized diff, timeout, or an unconfirmed Git
/// result) is a safe, content-free error and never a usable diff.
pub fn handle_get_user_diff_for_review(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> Result<RawUserDiffForReviewDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let ready = runtime.ready_snapshot()?;

    let mut candidate_port = GitCliAdapter::from_environment()
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    let mut filesystem = ready.filesystem.clone();
    let mut repository = ready.repository.clone();

    let review = UserDiffReviewReader::new(&mut repository, &mut filesystem, &mut candidate_port)
        .read_diff_for_review(&ReadUserDiffForReviewRequest::new(id, expected_version))
        .map_err(IpcErrorDto::from)?;

    Ok(RawUserDiffForReviewDto::from(review))
}

/// Recomputes the task's current worktree diff and content hash, and only
/// if it exactly matches `expected_diff_content_hash` records or reuses a
/// content-free, hash-bound approval. Never accepts raw diff text as input
/// and never returns any in its response. A malformed `expected_diff_content_hash`
/// (not exactly 64 lowercase hex characters) is rejected before any Git
/// process or repository write. On a hash mismatch, stale version, invalid
/// isolation, or diff read failure, no approval row is created and the
/// task's state/version are left untouched.
pub fn handle_approve_user_diff(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
    expected_diff_content_hash: &str,
) -> Result<UserDiffApprovalDto, IpcErrorDto> {
    let id = parse_task_id(task_id)?;
    let expected_hash =
        DiffContentHash::from_hex(expected_diff_content_hash).ok_or_else(invalid_hash_error)?;
    let ready = runtime.ready_snapshot()?;

    let mut candidate_port = GitCliAdapter::from_environment()
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    let mut filesystem = ready.filesystem.clone();
    let mut repository = ready.repository.clone();
    let mut time = ready.time.clone();

    let view = UserDiffApprovalService::new(
        &mut repository,
        &mut time,
        &mut filesystem,
        &mut candidate_port,
    )
    .approve(ApproveUserDiffRequest::new(
        id,
        expected_version,
        expected_hash,
    ))
    .map_err(IpcErrorDto::from)?;

    Ok(UserDiffApprovalDto::from(view))
}

fn invalid_hash_error() -> IpcErrorDto {
    ApplicationError::from_failure(
        FailureCategory::InvalidInput,
        FailureCategory::InvalidInput.default_severity(),
        FailureCategory::InvalidInput.default_retry(),
    )
    .into()
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_user_diff_for_review(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> Result<RawUserDiffForReviewDto, IpcErrorDto> {
    handle_get_user_diff_for_review(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn approve_user_diff(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
    expected_diff_content_hash: String,
) -> Result<UserDiffApprovalDto, IpcErrorDto> {
    handle_approve_user_diff(
        &state,
        &task_id,
        expected_version,
        &expected_diff_content_hash,
    )
}
