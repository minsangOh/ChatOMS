use chatoms_application::tasks::{HighRiskApprovalStatus, HighRiskApprovalView};
use chatoms_domain::HighRiskCategory;
use serde::{Deserialize, Serialize};

/// Exhaustive one-to-one mirror of `chatoms_domain::HighRiskCategory`'s 13
/// fixed categories. Frontend/backend boundary literal only -- carries no
/// free-text description, no provider/work-kind/data-scope, and no
/// raw diff/plan/provider-output/path/auth/session/cost field. `Deserialize`
/// rejects any string outside these 13 fixed variants (fail-closed on
/// unknown/malformed input) rather than defaulting to a category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HighRiskCategoryDto {
    ArchitectureChange,
    DatabaseSchemaChange,
    AuthenticationOrAuthorizationChange,
    SecurityPolicyChange,
    ExternalNetworkBehaviorAddition,
    ExternalDataTransmissionAddition,
    LargeScaleFileMoveOrDeletion,
    PublicApiOrStorageFormatChange,
    OperatingSystemConfigurationChange,
    AdministratorPrivilegesRequired,
    BreakingCompatibilityChange,
    DataMigration,
    DifficultToRecoverChange,
}

impl From<HighRiskCategory> for HighRiskCategoryDto {
    fn from(value: HighRiskCategory) -> Self {
        match value {
            HighRiskCategory::ArchitectureChange => Self::ArchitectureChange,
            HighRiskCategory::DatabaseSchemaChange => Self::DatabaseSchemaChange,
            HighRiskCategory::AuthenticationOrAuthorizationChange => {
                Self::AuthenticationOrAuthorizationChange
            }
            HighRiskCategory::SecurityPolicyChange => Self::SecurityPolicyChange,
            HighRiskCategory::ExternalNetworkBehaviorAddition => {
                Self::ExternalNetworkBehaviorAddition
            }
            HighRiskCategory::ExternalDataTransmissionAddition => {
                Self::ExternalDataTransmissionAddition
            }
            HighRiskCategory::LargeScaleFileMoveOrDeletion => Self::LargeScaleFileMoveOrDeletion,
            HighRiskCategory::PublicApiOrStorageFormatChange => {
                Self::PublicApiOrStorageFormatChange
            }
            HighRiskCategory::OperatingSystemConfigurationChange => {
                Self::OperatingSystemConfigurationChange
            }
            HighRiskCategory::AdministratorPrivilegesRequired => {
                Self::AdministratorPrivilegesRequired
            }
            HighRiskCategory::BreakingCompatibilityChange => Self::BreakingCompatibilityChange,
            HighRiskCategory::DataMigration => Self::DataMigration,
            HighRiskCategory::DifficultToRecoverChange => Self::DifficultToRecoverChange,
        }
    }
}

impl From<HighRiskCategoryDto> for HighRiskCategory {
    fn from(value: HighRiskCategoryDto) -> Self {
        match value {
            HighRiskCategoryDto::ArchitectureChange => Self::ArchitectureChange,
            HighRiskCategoryDto::DatabaseSchemaChange => Self::DatabaseSchemaChange,
            HighRiskCategoryDto::AuthenticationOrAuthorizationChange => {
                Self::AuthenticationOrAuthorizationChange
            }
            HighRiskCategoryDto::SecurityPolicyChange => Self::SecurityPolicyChange,
            HighRiskCategoryDto::ExternalNetworkBehaviorAddition => {
                Self::ExternalNetworkBehaviorAddition
            }
            HighRiskCategoryDto::ExternalDataTransmissionAddition => {
                Self::ExternalDataTransmissionAddition
            }
            HighRiskCategoryDto::LargeScaleFileMoveOrDeletion => Self::LargeScaleFileMoveOrDeletion,
            HighRiskCategoryDto::PublicApiOrStorageFormatChange => {
                Self::PublicApiOrStorageFormatChange
            }
            HighRiskCategoryDto::OperatingSystemConfigurationChange => {
                Self::OperatingSystemConfigurationChange
            }
            HighRiskCategoryDto::AdministratorPrivilegesRequired => {
                Self::AdministratorPrivilegesRequired
            }
            HighRiskCategoryDto::BreakingCompatibilityChange => Self::BreakingCompatibilityChange,
            HighRiskCategoryDto::DataMigration => Self::DataMigration,
            HighRiskCategoryDto::DifficultToRecoverChange => Self::DifficultToRecoverChange,
        }
    }
}

/// Content-free read-only status: whether an exact `(task_id,
/// expected_version, risk_category)` high-risk approval already exists.
/// Carries nothing else -- no timestamp, no task identity, no diff/plan
/// content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HighRiskApprovalStatusDto {
    pub approved: bool,
}

impl From<HighRiskApprovalStatus> for HighRiskApprovalStatusDto {
    fn from(value: HighRiskApprovalStatus) -> Self {
        Self {
            approved: value.approved,
        }
    }
}

/// Content-free approval result: which category was approved and when.
/// Never echoes back the task id, provider, work kind, data scope, or any
/// source/diff/plan content -- the caller already knows which task/version
/// it asked about.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HighRiskApprovalDto {
    pub risk_category: HighRiskCategoryDto,
    pub approved_at_ms: i64,
}

impl From<HighRiskApprovalView> for HighRiskApprovalDto {
    fn from(value: HighRiskApprovalView) -> Self {
        Self {
            risk_category: value.risk_category.into(),
            approved_at_ms: value.approved_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::de::{
        IntoDeserializer,
        value::{Error as DeError, StrDeserializer},
    };
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    const ALL_CATEGORIES: [HighRiskCategoryDto; 13] = [
        HighRiskCategoryDto::ArchitectureChange,
        HighRiskCategoryDto::DatabaseSchemaChange,
        HighRiskCategoryDto::AuthenticationOrAuthorizationChange,
        HighRiskCategoryDto::SecurityPolicyChange,
        HighRiskCategoryDto::ExternalNetworkBehaviorAddition,
        HighRiskCategoryDto::ExternalDataTransmissionAddition,
        HighRiskCategoryDto::LargeScaleFileMoveOrDeletion,
        HighRiskCategoryDto::PublicApiOrStorageFormatChange,
        HighRiskCategoryDto::OperatingSystemConfigurationChange,
        HighRiskCategoryDto::AdministratorPrivilegesRequired,
        HighRiskCategoryDto::BreakingCompatibilityChange,
        HighRiskCategoryDto::DataMigration,
        HighRiskCategoryDto::DifficultToRecoverChange,
    ];

    fn json(response: impl IpcResponse) -> String {
        let InvokeResponseBody::Json(json) = response.body().expect("JSON serialization") else {
            panic!("expected JSON response");
        };
        json
    }

    fn deserialize_category_str(value: &str) -> Result<HighRiskCategoryDto, DeError> {
        let deserializer: StrDeserializer<'_, DeError> = value.into_deserializer();
        HighRiskCategoryDto::deserialize(deserializer)
    }

    #[test]
    fn all_thirteen_categories_round_trip_through_domain_conversion() {
        for dto in ALL_CATEGORIES {
            let domain: HighRiskCategory = dto.into();
            let back: HighRiskCategoryDto = domain.into();
            assert_eq!(dto, back, "round trip must preserve the exact category");
        }
    }

    #[test]
    fn all_thirteen_categories_serialize_to_the_expected_fixed_camel_case_literal_and_round_trip() {
        let expected = [
            (
                HighRiskCategoryDto::ArchitectureChange,
                "architectureChange",
            ),
            (
                HighRiskCategoryDto::DatabaseSchemaChange,
                "databaseSchemaChange",
            ),
            (
                HighRiskCategoryDto::AuthenticationOrAuthorizationChange,
                "authenticationOrAuthorizationChange",
            ),
            (
                HighRiskCategoryDto::SecurityPolicyChange,
                "securityPolicyChange",
            ),
            (
                HighRiskCategoryDto::ExternalNetworkBehaviorAddition,
                "externalNetworkBehaviorAddition",
            ),
            (
                HighRiskCategoryDto::ExternalDataTransmissionAddition,
                "externalDataTransmissionAddition",
            ),
            (
                HighRiskCategoryDto::LargeScaleFileMoveOrDeletion,
                "largeScaleFileMoveOrDeletion",
            ),
            (
                HighRiskCategoryDto::PublicApiOrStorageFormatChange,
                "publicApiOrStorageFormatChange",
            ),
            (
                HighRiskCategoryDto::OperatingSystemConfigurationChange,
                "operatingSystemConfigurationChange",
            ),
            (
                HighRiskCategoryDto::AdministratorPrivilegesRequired,
                "administratorPrivilegesRequired",
            ),
            (
                HighRiskCategoryDto::BreakingCompatibilityChange,
                "breakingCompatibilityChange",
            ),
            (HighRiskCategoryDto::DataMigration, "dataMigration"),
            (
                HighRiskCategoryDto::DifficultToRecoverChange,
                "difficultToRecoverChange",
            ),
        ];
        for (dto, expected_literal) in expected {
            assert_eq!(json(dto), format!("\"{expected_literal}\""));
            assert_eq!(
                deserialize_category_str(expected_literal).expect("deserialize known literal"),
                dto
            );
        }
    }

    #[test]
    fn unknown_category_string_fails_closed_on_deserialize() {
        for malformed in [
            "NotACategory",
            "architecturechange",
            "",
            "ArchitectureChange ",
        ] {
            assert!(
                deserialize_category_str(malformed).is_err(),
                "must reject malformed input: {malformed:?}"
            );
        }
    }

    #[test]
    fn status_dto_serializes_only_the_approved_field() {
        assert_eq!(
            json(HighRiskApprovalStatusDto { approved: true }),
            "{\"approved\":true}"
        );
        assert_eq!(
            json(HighRiskApprovalStatusDto { approved: false }),
            "{\"approved\":false}"
        );
    }

    #[test]
    fn approval_dto_serializes_only_risk_category_and_approved_at_ms() {
        let dto = HighRiskApprovalDto {
            risk_category: HighRiskCategoryDto::DataMigration,
            approved_at_ms: 12345,
        };
        assert_eq!(
            json(dto),
            "{\"riskCategory\":\"dataMigration\",\"approvedAtMs\":12345}"
        );
    }
}
