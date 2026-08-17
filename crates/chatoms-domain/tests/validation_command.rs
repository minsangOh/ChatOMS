use chatoms_domain::{ValidationCommandKind, ValidationExecutionScope};

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

#[test]
fn validation_execution_scope_is_closed_to_worktree_and_project_root() {
    assert_eq!(
        ValidationExecutionScope::ALL,
        [
            ValidationExecutionScope::TaskWorktree,
            ValidationExecutionScope::ProjectRoot,
        ]
    );
}
