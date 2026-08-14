use chatoms_domain::ValidationCommandKind;

#[test]
fn all_lists_exactly_the_five_product_requirements_categories() {
    assert_eq!(
        ValidationCommandKind::ALL,
        [
            ValidationCommandKind::Format,
            ValidationCommandKind::Lint,
            ValidationCommandKind::Typecheck,
            ValidationCommandKind::Test,
            ValidationCommandKind::Build,
        ]
    );
}
