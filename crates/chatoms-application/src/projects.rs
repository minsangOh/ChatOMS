use chatoms_domain::ProjectId;
use chatoms_ports::repository::{FoundationRepository, ProjectSummary};

use crate::error::ApplicationError;

#[derive(Clone, Eq, PartialEq)]
pub struct ProjectView {
    id: ProjectId,
    name: String,
    root_path: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl ProjectView {
    #[must_use]
    pub const fn id(&self) -> ProjectId {
        self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn root_path(&self) -> &str {
        &self.root_path
    }

    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    #[must_use]
    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
}

impl From<ProjectSummary> for ProjectView {
    fn from(project: ProjectSummary) -> Self {
        Self {
            id: project.id,
            name: project.name,
            root_path: project.root_path,
            created_at_ms: project.created_at_ms,
            updated_at_ms: project.updated_at_ms,
        }
    }
}

pub struct ProjectService<'a, R> {
    repository: &'a mut R,
}

impl<'a, R> ProjectService<'a, R>
where
    R: FoundationRepository,
{
    #[must_use]
    pub const fn new(repository: &'a mut R) -> Self {
        Self { repository }
    }

    pub fn list_projects(&mut self) -> Result<Vec<ProjectView>, ApplicationError> {
        let mut projects = self
            .repository
            .list_projects()
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .into_iter()
            .map(ProjectView::from)
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
        });
        Ok(projects)
    }
}
