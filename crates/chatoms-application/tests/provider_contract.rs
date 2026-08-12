use std::{ffi::OsString, path::PathBuf};

use chatoms_application::{
    error::{ApplicationError, ApplicationErrorCode},
    system::{CapabilityStatus, ProviderCapabilitySummary},
};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    process::{ProcessOutcome, ProcessSpec},
    provider::{
        ProviderCapabilities, ProviderCapabilityPort, ProviderCapabilityStatus, ProviderKind,
    },
};

struct ProviderCapabilityFake(Result<ProviderCapabilities, PortFailure>);

impl ProviderCapabilityPort for ProviderCapabilityFake {
    fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure> {
        self.0
    }
}

#[test]
fn provider_and_process_vocabulary_needs_only_std_and_ports_crates() {
    let capabilities = ProviderCapabilities {
        claude: ProviderCapabilityStatus::Supported,
        codex: ProviderCapabilityStatus::Unsupported,
    };
    let mut fake = ProviderCapabilityFake(Ok(capabilities));
    assert_eq!(
        fake.provider_capabilities().expect("capabilities"),
        capabilities
    );
    assert_ne!(ProviderKind::Claude, ProviderKind::Codex);

    let spec = ProcessSpec {
        executable: PathBuf::from("provider.exe"),
        arguments: vec![OsString::from("--version")],
        working_directory: PathBuf::from("."),
    };
    assert_eq!(spec.clone(), spec);
    assert_ne!(ProcessOutcome::Completed, ProcessOutcome::Uncertain);
}

#[test]
fn provider_capability_failure_maps_to_the_same_safe_application_error_as_platform_capability() {
    let mut fake = ProviderCapabilityFake(Err(PortFailure::new(FailureCategory::Unsupported)));
    let error = fake
        .provider_capabilities()
        .map_err(|failure| ApplicationError::from_categorized(&failure))
        .expect_err("provider capability failure");
    assert_eq!(error.code(), ApplicationErrorCode::Unsupported);
    assert_eq!(
        error.to_string(),
        "APP_UNSUPPORTED: This operation is not supported on the current platform."
    );
}

#[test]
fn provider_capability_summary_defaults_fail_closed_and_reuses_the_existing_capability_status() {
    let placeholder = ProviderCapabilitySummary::not_yet_probed();
    assert_eq!(placeholder.claude, None);
    assert_eq!(placeholder.codex, None);

    let probed = ProviderCapabilitySummary {
        claude: Some(CapabilityStatus::Supported),
        codex: Some(CapabilityStatus::Unsupported),
    };
    assert_eq!(probed.claude, Some(CapabilityStatus::Supported));
    assert_eq!(probed.codex, Some(CapabilityStatus::Unsupported));
}
