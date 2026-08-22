use std::collections::HashSet;

use chatoms_application::{
    error::{ApplicationError, ApplicationErrorCode},
    operation_risk_declaration::{
        DeclareProviderImplementationRiskRequest, OperationRiskDeclarationService,
    },
};
use chatoms_domain::{HighRiskCategory, OperationRiskKind, TaskId, TaskState};
use chatoms_ports::{TimeProvider, error::FailureCategory, repository::FoundationRepository};

use crate::{
    dto::{
        HighRiskCategoryDto, OperationRiskApprovalReadinessDto,
        OperationRiskAssessmentFailureCategoryDto, OperationRiskAssessmentStatusDto,
    },
    error::IpcErrorDto,
    state::ManagedRuntime,
};

use super::tasks::parse_task_id;

pub fn handle_get_provider_implementation_risk_assessment_status(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
) -> OperationRiskAssessmentStatusDto {
    let id = match parse_task_id(task_id) {
        Ok(id) => id,
        Err(error) => return failed_ipc(error),
    };
    let mut ready = match runtime.ready_snapshot() {
        Ok(ready) => ready,
        Err(error) => return failed_ipc(error),
    };
    match load_status(&mut ready.repository, id, expected_version) {
        Ok(status) => status,
        Err(error) => failed_application(error),
    }
}

pub fn handle_declare_provider_implementation_risk(
    runtime: &ManagedRuntime,
    task_id: &str,
    expected_version: u64,
    risk_categories: Vec<HighRiskCategoryDto>,
    explicit_empty: bool,
) -> OperationRiskAssessmentStatusDto {
    if explicit_empty != risk_categories.is_empty()
        || risk_categories
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != risk_categories.len()
    {
        return OperationRiskAssessmentStatusDto::failed(
            OperationRiskAssessmentFailureCategoryDto::InvalidInput,
        );
    }
    let id = match parse_task_id(task_id) {
        Ok(id) => id,
        Err(error) => return failed_ipc(error),
    };
    let mut ready = match runtime.ready_snapshot() {
        Ok(ready) => ready,
        Err(error) => return failed_ipc(error),
    };
    let declared_at_ms = match ready.time.now_ms() {
        Ok(value) => value,
        Err(_) => {
            return OperationRiskAssessmentStatusDto::failed(
                OperationRiskAssessmentFailureCategoryDto::Internal,
            );
        }
    };
    let domain_categories = risk_categories.into_iter().map(Into::into).collect();
    let declaration =
        OperationRiskDeclarationService::new(&mut ready.repository, &mut ready.filesystem)
            .declare_provider_implementation_risk(DeclareProviderImplementationRiskRequest {
                task_id: id,
                expected_version,
                risk_categories: domain_categories,
                declared_at_ms,
            });
    if let Err(error) = declaration {
        return failed_application(error);
    }
    match load_status(&mut ready.repository, id, expected_version) {
        Ok(status) => status,
        Err(error) => failed_application(error),
    }
}

fn load_status(
    repository: &mut impl FoundationRepository,
    task_id: TaskId,
    expected_version: u64,
) -> Result<OperationRiskAssessmentStatusDto, ApplicationError> {
    let task = repository
        .get_task(task_id)
        .map_err(|error| ApplicationError::from_categorized(&error))?
        .ok_or_else(|| category_error(FailureCategory::NotFound))?;
    if task.version() != expected_version {
        return Err(category_error(FailureCategory::VersionConflict));
    }
    if task.state() != TaskState::AwaitingDesignApproval {
        return Err(category_error(FailureCategory::InvalidState));
    }
    let declaration = repository
        .get_operation_risk_declaration(
            task_id,
            expected_version,
            OperationRiskKind::ProviderImplementation,
        )
        .map_err(|error| ApplicationError::from_categorized(&error))?;
    let mut approval_readiness = Vec::with_capacity(HighRiskCategory::ALL.len());
    for category in HighRiskCategory::ALL {
        let approved = repository
            .get_high_risk_approval(task_id, expected_version, category)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .is_some();
        approval_readiness.push(OperationRiskApprovalReadinessDto::new(category, approved));
    }
    let declaration_exists = declaration.is_some();
    let selected_categories = declaration
        .map(|value| value.risk_categories)
        .unwrap_or_default();
    Ok(OperationRiskAssessmentStatusDto::ready(
        selected_categories,
        approval_readiness,
        declaration_exists,
    ))
}

fn failed_application(error: ApplicationError) -> OperationRiskAssessmentStatusDto {
    OperationRiskAssessmentStatusDto::failed(error.code().into())
}

fn failed_ipc(error: IpcErrorDto) -> OperationRiskAssessmentStatusDto {
    let category = match error.code {
        "APP_INVALID_INPUT" => ApplicationErrorCode::InvalidInput,
        "APP_NOT_FOUND" => ApplicationErrorCode::NotFound,
        "APP_VERSION_CONFLICT" => ApplicationErrorCode::VersionConflict,
        "APP_INVALID_STATE" => ApplicationErrorCode::InvalidState,
        "APP_ACTIVE_TASK_CONFLICT" => ApplicationErrorCode::ActiveTaskConflict,
        "APP_CONFLICT" => ApplicationErrorCode::Conflict,
        "APP_STORAGE_UNAVAILABLE"
        | "APP_STORAGE_INSECURE"
        | "APP_PERMISSION_DENIED"
        | "APP_MIGRATION_FAILED" => ApplicationErrorCode::StorageUnavailable,
        _ => ApplicationErrorCode::Internal,
    };
    OperationRiskAssessmentStatusDto::failed(category.into())
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_provider_implementation_risk_assessment_status(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
) -> OperationRiskAssessmentStatusDto {
    handle_get_provider_implementation_risk_assessment_status(&state, &task_id, expected_version)
}

#[tauri::command(rename_all = "camelCase")]
pub fn declare_provider_implementation_risk(
    state: tauri::State<'_, ManagedRuntime>,
    task_id: String,
    expected_version: u64,
    risk_categories: Vec<HighRiskCategoryDto>,
    explicit_empty: bool,
) -> OperationRiskAssessmentStatusDto {
    handle_declare_provider_implementation_risk(
        &state,
        &task_id,
        expected_version,
        risk_categories,
        explicit_empty,
    )
}
