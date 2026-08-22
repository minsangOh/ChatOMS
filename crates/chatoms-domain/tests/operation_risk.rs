use chatoms_domain::{OperationRiskKind, TargetIdentityDigest};

#[test]
fn provider_implementation_is_the_only_persisted_operation_kind() {
    assert_eq!(
        OperationRiskKind::ALL,
        [OperationRiskKind::ProviderImplementation]
    );
    assert_eq!(
        OperationRiskKind::ProviderImplementation.persisted_text(),
        "ProviderImplementation"
    );
    assert_eq!(
        OperationRiskKind::from_persisted_text("ProviderImplementation"),
        Some(OperationRiskKind::ProviderImplementation)
    );
    assert_eq!(OperationRiskKind::from_persisted_text("Planning"), None);
}

#[test]
fn target_identity_digest_round_trips_lowercase_sha256_hex() {
    let digest = TargetIdentityDigest::from_digest_bytes([0xab; 32]);
    let hex = digest.to_hex();

    assert_eq!(hex, "ab".repeat(32));
    assert_eq!(TargetIdentityDigest::from_hex(&hex), Some(digest));
}

#[test]
fn target_identity_digest_rejects_noncanonical_hex() {
    for malformed in [
        "ab",
        "ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB",
        "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
    ] {
        assert_eq!(TargetIdentityDigest::from_hex(malformed), None);
    }
}
