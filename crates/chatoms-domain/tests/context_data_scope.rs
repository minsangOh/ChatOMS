use chatoms_domain::ContextDataScope;

#[test]
fn context_data_scope_vocabulary_is_exactly_two_fixed_variants() {
    assert_eq!(
        ContextDataScope::ALL,
        [
            ContextDataScope::LegacyPhase4,
            ContextDataScope::ContextPackageV1,
        ]
    );
}

#[test]
fn context_data_scope_variants_are_distinct() {
    assert_ne!(
        ContextDataScope::LegacyPhase4,
        ContextDataScope::ContextPackageV1
    );
}
