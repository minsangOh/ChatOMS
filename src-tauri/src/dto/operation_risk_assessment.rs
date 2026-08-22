use chatoms_application::error::ApplicationErrorCode;
use chatoms_domain::HighRiskCategory;
use serde::Serialize;

use super::HighRiskCategoryDto;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationRiskAssessmentFailureCategoryDto {
    InvalidInput,
    NotFound,
    VersionConflict,
    InvalidState,
    ActiveLeaseConflict,
    IdentityMismatch,
    PersistenceUnavailable,
    Internal,
}

impl From<ApplicationErrorCode> for OperationRiskAssessmentFailureCategoryDto {
    fn from(value: ApplicationErrorCode) -> Self {
        match value {
            ApplicationErrorCode::InvalidInput => Self::InvalidInput,
            ApplicationErrorCode::NotFound => Self::NotFound,
            ApplicationErrorCode::VersionConflict => Self::VersionConflict,
            ApplicationErrorCode::InvalidState => Self::InvalidState,
            ApplicationErrorCode::ActiveTaskConflict => Self::ActiveLeaseConflict,
            ApplicationErrorCode::Conflict => Self::IdentityMismatch,
            ApplicationErrorCode::AlreadyExists
            | ApplicationErrorCode::StorageUnavailable
            | ApplicationErrorCode::StorageInsecure
            | ApplicationErrorCode::PermissionDenied
            | ApplicationErrorCode::MigrationFailed => Self::PersistenceUnavailable,
            ApplicationErrorCode::SequenceConflict
            | ApplicationErrorCode::RedactionFailed
            | ApplicationErrorCode::LoggingUnavailable
            | ApplicationErrorCode::Unsupported
            | ApplicationErrorCode::Internal => Self::Internal,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRiskApprovalReadinessDto {
    pub risk_category: HighRiskCategoryDto,
    pub approved: bool,
}

impl OperationRiskApprovalReadinessDto {
    pub fn new(risk_category: HighRiskCategory, approved: bool) -> Self {
        Self {
            risk_category: risk_category.into(),
            approved,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRiskAssessmentStatusDto {
    pub assessment_required: Option<bool>,
    pub declaration_exists: Option<bool>,
    pub selected_categories: Vec<HighRiskCategoryDto>,
    pub approval_readiness: Vec<OperationRiskApprovalReadinessDto>,
    pub failure_category: Option<OperationRiskAssessmentFailureCategoryDto>,
}

impl OperationRiskAssessmentStatusDto {
    pub fn ready(
        selected_categories: Vec<HighRiskCategory>,
        approval_readiness: Vec<OperationRiskApprovalReadinessDto>,
        declaration_exists: bool,
    ) -> Self {
        Self {
            assessment_required: Some(!declaration_exists),
            declaration_exists: Some(declaration_exists),
            selected_categories: selected_categories.into_iter().map(Into::into).collect(),
            approval_readiness,
            failure_category: None,
        }
    }

    pub fn failed(failure_category: OperationRiskAssessmentFailureCategoryDto) -> Self {
        Self {
            assessment_required: None,
            declaration_exists: None,
            selected_categories: Vec::new(),
            approval_readiness: Vec::new(),
            failure_category: Some(failure_category),
        }
    }
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    fn json(response: impl IpcResponse) -> String {
        let InvokeResponseBody::Json(json) = response.body().expect("JSON serialization") else {
            panic!("expected JSON response");
        };
        json
    }

    #[test]
    fn ready_status_serializes_only_the_content_free_contract() {
        let response = OperationRiskAssessmentStatusDto::ready(
            vec![HighRiskCategory::DataMigration],
            vec![OperationRiskApprovalReadinessDto::new(
                HighRiskCategory::DataMigration,
                true,
            )],
            true,
        );
        let body = json(response);
        assert_eq!(
            body,
            r#"{"assessmentRequired":false,"declarationExists":true,"selectedCategories":["dataMigration"],"approvalReadiness":[{"riskCategory":"dataMigration","approved":true}],"failureCategory":null}"#
        );
        for forbidden in ["path", "digest", "stdout", "operation", "plan", "prompt"] {
            assert!(!body.contains(forbidden));
        }
    }

    #[test]
    fn failure_status_contains_only_the_fixed_category() {
        let body = json(OperationRiskAssessmentStatusDto::failed(
            OperationRiskAssessmentFailureCategoryDto::IdentityMismatch,
        ));
        assert_eq!(
            body,
            r#"{"assessmentRequired":null,"declarationExists":null,"selectedCategories":[],"approvalReadiness":[],"failureCategory":"identityMismatch"}"#
        );
    }
}
