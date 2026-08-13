use chatoms_application::{
    provider_eligibility::{
        ContractStatus, EligibilityBlockingReason, EligibilityCapability, ProviderEligibilityPolicy,
    },
    system::{CapabilityStatus, ProviderCapabilitySummary},
};
use chatoms_domain::{TaskState, WorkKind};
use chatoms_ports::provider::ProviderKind;

#[test]
fn provider_work_kind_capability_truth_table_is_fail_closed() {
    for cached_capability in [
        Some(CapabilityStatus::Supported),
        Some(CapabilityStatus::Unsupported),
        None,
    ] {
        let capability_summary = ProviderCapabilitySummary {
            claude: cached_capability,
            codex: cached_capability,
        };
        for work_kind in WorkKind::ALL {
            let entries =
                ProviderEligibilityPolicy::evaluate(work_kind.entry_state(), capability_summary);
            for provider in [ProviderKind::Claude, ProviderKind::Codex] {
                let entry = entries
                    .iter()
                    .find(|entry| entry.work_kind == work_kind && entry.provider == provider)
                    .expect("provider and work kind entry");
                let expected_contract = match (provider, work_kind) {
                    (ProviderKind::Claude, WorkKind::Planning | WorkKind::Review) => {
                        ContractStatus::Approved
                    }
                    (ProviderKind::Claude, WorkKind::Implementation) | (ProviderKind::Codex, _) => {
                        ContractStatus::NotApproved
                    }
                };
                let expected_capability = match cached_capability {
                    Some(CapabilityStatus::Supported) => EligibilityCapability::Supported,
                    Some(CapabilityStatus::Unsupported) => EligibilityCapability::Unsupported,
                    None => EligibilityCapability::Unavailable,
                };
                assert_eq!(entry.contract, expected_contract);
                assert_eq!(entry.capability, expected_capability);
                assert_eq!(
                    entry.eligible,
                    expected_capability == EligibilityCapability::Supported
                        && expected_contract == ContractStatus::Approved
                );
                assert!(entry.state_allows_work_kind);
                assert_eq!(
                    entry.blocking_reasons,
                    expected_blocking_reasons(expected_capability, expected_contract, true)
                );
            }
        }
    }
}

#[test]
fn task_state_mismatch_is_reported_without_changing_provider_eligibility() {
    let entries = ProviderEligibilityPolicy::evaluate(
        TaskState::Created,
        ProviderCapabilitySummary {
            claude: Some(CapabilityStatus::Supported),
            codex: Some(CapabilityStatus::Unsupported),
        },
    );

    assert_eq!(entries.len(), 6);
    for entry in &entries {
        assert!(!entry.state_allows_work_kind);
        assert!(
            entry
                .blocking_reasons
                .contains(&EligibilityBlockingReason::TaskStateMismatch)
        );
    }
    assert!(entry(&entries, WorkKind::Planning, ProviderKind::Claude).eligible);
    assert!(entry(&entries, WorkKind::Review, ProviderKind::Claude).eligible);
    assert!(!entry(&entries, WorkKind::Implementation, ProviderKind::Claude).eligible);
    assert!(!entry(&entries, WorkKind::Planning, ProviderKind::Codex).eligible);
}

fn entry(
    entries: &[chatoms_application::provider_eligibility::ProviderEligibilityView],
    work_kind: WorkKind,
    provider: ProviderKind,
) -> &chatoms_application::provider_eligibility::ProviderEligibilityView {
    entries
        .iter()
        .find(|entry| entry.work_kind == work_kind && entry.provider == provider)
        .expect("provider eligibility entry")
}

fn expected_blocking_reasons(
    capability: EligibilityCapability,
    contract: ContractStatus,
    state_allows_work_kind: bool,
) -> Vec<EligibilityBlockingReason> {
    let mut reasons = match capability {
        EligibilityCapability::Supported => Vec::new(),
        EligibilityCapability::Unsupported => {
            vec![EligibilityBlockingReason::CapabilityUnsupported]
        }
        EligibilityCapability::Unavailable => {
            vec![EligibilityBlockingReason::CapabilityUnavailable]
        }
    };
    if contract == ContractStatus::NotApproved {
        reasons.push(EligibilityBlockingReason::ContractNotApproved);
    }
    if !state_allows_work_kind {
        reasons.push(EligibilityBlockingReason::TaskStateMismatch);
    }
    reasons
}
