use chatoms_application::provider_eligibility::{
    ContractStatus, EligibilityBlockingReason, EligibilityCapability, ProviderEligibilityView,
};
use chatoms_domain::WorkKind;
use chatoms_ports::provider::ProviderKind;
use serde::Serialize;

use super::CapabilityStatusDto;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkKindDto {
    Planning,
    Implementation,
    Review,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKindDto {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ContractStatusDto {
    Approved,
    NotApproved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EligibilityBlockingReasonDto {
    CapabilityUnavailable,
    CapabilityUnsupported,
    ContractNotApproved,
    TaskStateMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEligibilityDto {
    pub work_kind: WorkKindDto,
    pub provider: ProviderKindDto,
    pub capability: CapabilityStatusDto,
    pub contract: ContractStatusDto,
    pub eligible: bool,
    pub state_allows_work_kind: bool,
    pub blocking_reasons: Vec<EligibilityBlockingReasonDto>,
}

impl From<ProviderEligibilityView> for ProviderEligibilityDto {
    fn from(value: ProviderEligibilityView) -> Self {
        Self {
            work_kind: value.work_kind.into(),
            provider: value.provider.into(),
            capability: value.capability.into(),
            contract: value.contract.into(),
            eligible: value.eligible,
            state_allows_work_kind: value.state_allows_work_kind,
            blocking_reasons: value
                .blocking_reasons
                .into_iter()
                .map(EligibilityBlockingReasonDto::from)
                .collect(),
        }
    }
}

impl From<WorkKind> for WorkKindDto {
    fn from(value: WorkKind) -> Self {
        match value {
            WorkKind::Planning => Self::Planning,
            WorkKind::Implementation => Self::Implementation,
            WorkKind::Review => Self::Review,
        }
    }
}

impl From<ProviderKind> for ProviderKindDto {
    fn from(value: ProviderKind) -> Self {
        match value {
            ProviderKind::Claude => Self::Claude,
            ProviderKind::Codex => Self::Codex,
        }
    }
}

impl From<EligibilityCapability> for CapabilityStatusDto {
    fn from(value: EligibilityCapability) -> Self {
        match value {
            EligibilityCapability::Supported => Self::Supported,
            EligibilityCapability::Unsupported => Self::Unsupported,
            EligibilityCapability::Unavailable => Self::Unavailable,
        }
    }
}

impl From<ContractStatus> for ContractStatusDto {
    fn from(value: ContractStatus) -> Self {
        match value {
            ContractStatus::Approved => Self::Approved,
            ContractStatus::NotApproved => Self::NotApproved,
        }
    }
}

impl From<EligibilityBlockingReason> for EligibilityBlockingReasonDto {
    fn from(value: EligibilityBlockingReason) -> Self {
        match value {
            EligibilityBlockingReason::CapabilityUnavailable => Self::CapabilityUnavailable,
            EligibilityBlockingReason::CapabilityUnsupported => Self::CapabilityUnsupported,
            EligibilityBlockingReason::ContractNotApproved => Self::ContractNotApproved,
            EligibilityBlockingReason::TaskStateMismatch => Self::TaskStateMismatch,
        }
    }
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn eligibility_dto_serializes_only_the_approved_safe_fields() {
        let dto = ProviderEligibilityDto::from(ProviderEligibilityView {
            work_kind: WorkKind::Planning,
            provider: ProviderKind::Claude,
            capability: EligibilityCapability::Unavailable,
            contract: ContractStatus::Approved,
            eligible: false,
            state_allows_work_kind: false,
            blocking_reasons: vec![
                EligibilityBlockingReason::CapabilityUnavailable,
                EligibilityBlockingReason::TaskStateMismatch,
            ],
        });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize eligibility DTO") else {
            panic!("expected JSON response");
        };
        assert_eq!(
            json,
            "{\"workKind\":\"planning\",\"provider\":\"claude\",\"capability\":\"unavailable\",\"contract\":\"approved\",\"eligible\":false,\"stateAllowsWorkKind\":false,\"blockingReasons\":[\"capabilityUnavailable\",\"taskStateMismatch\"]}"
        );
    }
}
