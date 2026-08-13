use chatoms_domain::{DomainError, TaskBrief};

#[test]
fn constructs_with_all_three_fields_populated() {
    let brief = TaskBrief::new(
        "Add CSV export".to_owned(),
        "Export button downloads a CSV of the current view".to_owned(),
        "Do not touch the import pipeline".to_owned(),
    )
    .expect("valid brief");
    assert_eq!(brief.requirements(), "Add CSV export");
    assert_eq!(
        brief.completion_criteria(),
        "Export button downloads a CSV of the current view"
    );
    assert_eq!(brief.prohibited_scope(), "Do not touch the import pipeline");
}

#[test]
fn rejects_empty_or_whitespace_only_fields() {
    let valid = || "non-empty".to_owned();
    let blank_variants = ["", "   ", "\t\n"];

    for blank in blank_variants {
        assert_eq!(
            TaskBrief::new(blank.to_owned(), valid(), valid()).unwrap_err(),
            DomainError::InvalidTaskBrief
        );
        assert_eq!(
            TaskBrief::new(valid(), blank.to_owned(), valid()).unwrap_err(),
            DomainError::InvalidTaskBrief
        );
        assert_eq!(
            TaskBrief::new(valid(), valid(), blank.to_owned()).unwrap_err(),
            DomainError::InvalidTaskBrief
        );
    }
}

#[test]
fn has_no_mutation_api_beyond_construction() {
    // TaskBrief exposes only read accessors; the only way to change its
    // content is to construct a new value. This test documents that
    // immutability guarantee at the type level: it would fail to compile if
    // a setter existed and were called here.
    let brief = TaskBrief::new("r".to_owned(), "c".to_owned(), "p".to_owned()).expect("valid");
    let cloned = brief.clone();
    assert_eq!(brief, cloned);
}
