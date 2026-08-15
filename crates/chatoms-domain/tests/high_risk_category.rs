use chatoms_domain::HighRiskCategory;

#[test]
fn high_risk_category_vocabulary_is_exactly_thirteen_fixed_variants() {
    assert_eq!(
        HighRiskCategory::ALL,
        [
            HighRiskCategory::ArchitectureChange,
            HighRiskCategory::DatabaseSchemaChange,
            HighRiskCategory::AuthenticationOrAuthorizationChange,
            HighRiskCategory::SecurityPolicyChange,
            HighRiskCategory::ExternalNetworkBehaviorAddition,
            HighRiskCategory::ExternalDataTransmissionAddition,
            HighRiskCategory::LargeScaleFileMoveOrDeletion,
            HighRiskCategory::PublicApiOrStorageFormatChange,
            HighRiskCategory::OperatingSystemConfigurationChange,
            HighRiskCategory::AdministratorPrivilegesRequired,
            HighRiskCategory::BreakingCompatibilityChange,
            HighRiskCategory::DataMigration,
            HighRiskCategory::DifficultToRecoverChange,
        ]
    );
}

#[test]
fn high_risk_category_variants_are_pairwise_distinct() {
    let all = HighRiskCategory::ALL;
    for (left_index, left) in all.iter().enumerate() {
        for (right_index, right) in all.iter().enumerate() {
            if left_index != right_index {
                assert_ne!(
                    left, right,
                    "duplicate category at {left_index}/{right_index}"
                );
            }
        }
    }
}

#[test]
fn high_risk_category_persisted_text_round_trips_exhaustively() {
    for category in HighRiskCategory::ALL {
        let text = category.persisted_text();
        assert_eq!(
            HighRiskCategory::from_persisted_text(text),
            Some(category),
            "round trip failed for {text}"
        );
    }
}

#[test]
fn high_risk_category_persisted_text_values_are_pairwise_distinct() {
    let all = HighRiskCategory::ALL;
    for (left_index, left) in all.iter().enumerate() {
        for (right_index, right) in all.iter().enumerate() {
            if left_index != right_index {
                assert_ne!(
                    left.persisted_text(),
                    right.persisted_text(),
                    "duplicate persisted text at {left_index}/{right_index}"
                );
            }
        }
    }
}

#[test]
fn high_risk_category_from_persisted_text_rejects_unknown_values() {
    for unknown in [
        "",
        "architecturechange",
        "ArchitectureChange ",
        "NotACategory",
    ] {
        assert_eq!(HighRiskCategory::from_persisted_text(unknown), None);
    }
}
