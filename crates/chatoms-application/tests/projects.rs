mod support;

use chatoms_application::{error::ApplicationErrorCode, projects::ProjectService};
use chatoms_ports::repository::RepositoryErrorCode;

use support::{FakeRepository, project};

#[test]
fn empty_project_list_is_returned_without_adapter_types() {
    let mut repository = FakeRepository::default();
    let projects = ProjectService::new(&mut repository)
        .list_projects()
        .expect("empty list");
    assert!(projects.is_empty());
    assert_eq!(repository.calls, ["list_projects"]);
    assert!(
        std::any::type_name_of_val(&projects)
            .contains("chatoms_application::projects::ProjectView")
    );
}

#[test]
fn project_fields_are_preserved_and_multiple_projects_are_sorted() {
    let zulu = project("Zulu", "C:\\private\\zulu", 30);
    let alpha = project("alpha", "C:\\private\\alpha", 10);
    let expected_alpha_id = alpha.id;
    let mut repository = FakeRepository {
        projects: vec![zulu, alpha],
        ..FakeRepository::default()
    };
    let projects = ProjectService::new(&mut repository)
        .list_projects()
        .expect("projects");
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].id(), expected_alpha_id);
    assert_eq!(projects[0].name(), "alpha");
    assert_eq!(projects[0].root_path(), "C:\\private\\alpha");
    assert_eq!(projects[0].created_at_ms(), 10);
    assert_eq!(projects[0].updated_at_ms(), 11);
    assert_eq!(projects[1].name(), "Zulu");
}

#[test]
fn repository_failures_map_without_exposing_project_path_or_source() {
    for (repository_code, expected_code) in [
        (
            RepositoryErrorCode::ProjectNotFound,
            ApplicationErrorCode::NotFound,
        ),
        (
            RepositoryErrorCode::OperationFailed,
            ApplicationErrorCode::Internal,
        ),
    ] {
        let mut repository = FakeRepository {
            fail_on: Some(("list_projects", repository_code)),
            ..FakeRepository::default()
        };
        let result = ProjectService::new(&mut repository).list_projects();
        let error = match result {
            Ok(_) => panic!("repository failure expected"),
            Err(error) => error,
        };
        assert_eq!(error.code(), expected_code);
        let displayed = error.to_string();
        for forbidden in ["C:\\private", "S-1-5-", "SELECT", "token"] {
            assert!(!displayed.contains(forbidden));
        }
    }
}
