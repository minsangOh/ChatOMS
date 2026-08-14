use chatoms_domain::{TaskState, WorkKind};
use chatoms_ports::provider::ProviderKind;

use crate::system::{CapabilityStatus, ProviderCapabilitySummary};

const PROVIDERS: [ProviderKind; 2] = [ProviderKind::Claude, ProviderKind::Codex];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EligibilityCapability {
    Supported,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractStatus {
    Approved,
    NotApproved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EligibilityBlockingReason {
    CapabilityUnavailable,
    CapabilityUnsupported,
    ContractNotApproved,
    TaskStateMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderEligibilityView {
    pub work_kind: WorkKind,
    pub provider: ProviderKind,
    pub capability: EligibilityCapability,
    pub contract: ContractStatus,
    pub eligible: bool,
    pub state_allows_work_kind: bool,
    pub blocking_reasons: Vec<EligibilityBlockingReason>,
}

pub struct ProviderEligibilityPolicy;

impl ProviderEligibilityPolicy {
    #[must_use]
    pub fn evaluate(
        task_state: TaskState,
        capabilities: ProviderCapabilitySummary,
    ) -> Vec<ProviderEligibilityView> {
        let mut entries = Vec::with_capacity(WorkKind::ALL.len() * PROVIDERS.len());
        for work_kind in WorkKind::ALL {
            for provider in PROVIDERS {
                let capability = capability_for(capabilities, provider);
                let contract = contract_for(provider, work_kind);
                let eligible = capability == EligibilityCapability::Supported
                    && contract == ContractStatus::Approved;
                let state_allows_work_kind = work_kind.can_start_from(task_state);
                entries.push(ProviderEligibilityView {
                    work_kind,
                    provider,
                    capability,
                    contract,
                    eligible,
                    state_allows_work_kind,
                    blocking_reasons: blocking_reasons(
                        capability,
                        contract,
                        state_allows_work_kind,
                    ),
                });
            }
        }
        entries
    }
}

const fn capability_for(
    capabilities: ProviderCapabilitySummary,
    provider: ProviderKind,
) -> EligibilityCapability {
    let capability = match provider {
        ProviderKind::Claude => capabilities.claude,
        ProviderKind::Codex => capabilities.codex,
    };
    match capability {
        Some(CapabilityStatus::Supported) => EligibilityCapability::Supported,
        Some(CapabilityStatus::Unsupported) => EligibilityCapability::Unsupported,
        None => EligibilityCapability::Unavailable,
    }
}

const fn contract_for(provider: ProviderKind, work_kind: WorkKind) -> ContractStatus {
    match (provider, work_kind) {
        (
            ProviderKind::Claude,
            WorkKind::Planning | WorkKind::Implementation | WorkKind::Review,
        ) => ContractStatus::Approved,
        (ProviderKind::Codex, _) => ContractStatus::NotApproved,
    }
}

fn blocking_reasons(
    capability: EligibilityCapability,
    contract: ContractStatus,
    state_allows_work_kind: bool,
) -> Vec<EligibilityBlockingReason> {
    let mut reasons = Vec::with_capacity(3);
    match capability {
        EligibilityCapability::Supported => {}
        EligibilityCapability::Unsupported => {
            reasons.push(EligibilityBlockingReason::CapabilityUnsupported);
        }
        EligibilityCapability::Unavailable => {
            reasons.push(EligibilityBlockingReason::CapabilityUnavailable);
        }
    }
    match contract {
        ContractStatus::Approved => {}
        ContractStatus::NotApproved => {
            reasons.push(EligibilityBlockingReason::ContractNotApproved);
        }
    }
    if !state_allows_work_kind {
        reasons.push(EligibilityBlockingReason::TaskStateMismatch);
    }
    reasons
}
