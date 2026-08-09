use std::path::Path;

use chatoms_domain::ProjectId;
use chatoms_ports::{
    TimeProvider,
    filesystem::{DirectoryIdentity, FilesystemIdentityPort},
    git::{GitService, RepositoryKind, RepositoryStatus},
    repository::{
        FoundationRepository, ProjectFilesystemIdentityRecord, ProjectRecord, ProjectSummary,
    },
};
use sha2::{Digest, Sha256};

use crate::error::ApplicationError;

#[derive(Clone, Eq, PartialEq)]
pub struct ProjectView {
    id: ProjectId,
    name: String,
    root_path: String,
    display_path: String,
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
    pub fn display_path(&self) -> &str {
        &self.display_path
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
            display_path: project.display_path,
            created_at_ms: project.created_at_ms,
            updated_at_ms: project.updated_at_ms,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectCandidateView {
    pub suggested_name: String,
    pub display_path: String,
    pub confirmation_token: String,
    pub repository_kind: RepositoryKind,
    pub repository_status: Option<RepositoryStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectStatusView {
    pub project_id: ProjectId,
    pub repository_kind: RepositoryKind,
    pub repository_status: Option<RepositoryStatus>,
}

pub struct RegisterProjectRequest {
    pub input_path: String,
    pub confirmation_token: String,
    pub name: Option<String>,
}

pub struct ProjectMutationService<'a, R, G, F, T> {
    repository: &'a mut R,
    git: &'a mut G,
    filesystem: &'a mut F,
    time: &'a mut T,
}

impl<'a, R, G, F, T> ProjectMutationService<'a, R, G, F, T>
where
    R: FoundationRepository,
    G: GitService,
    F: FilesystemIdentityPort,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        git: &'a mut G,
        filesystem: &'a mut F,
        time: &'a mut T,
    ) -> Self {
        Self {
            repository,
            git,
            filesystem,
            time,
        }
    }

    pub fn inspect_candidate(
        &mut self,
        input_path: &str,
    ) -> Result<ProjectCandidateView, ApplicationError> {
        if input_path.trim().is_empty() {
            return Err(invalid_input());
        }
        self.inspect_input_before_git(Path::new(input_path))?;
        let inspection = self
            .git
            .inspect_project(Path::new(input_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let (root_identity, common_identity) = self.inspect_filesystem_identity(&inspection)?;
        Ok(ProjectCandidateView {
            suggested_name: inspection.suggested_name,
            display_path: inspection.display_path,
            confirmation_token: identity_confirmation_token(
                &inspection.confirmation_token,
                &root_identity,
                common_identity.as_ref(),
            ),
            repository_kind: inspection.repository_kind,
            repository_status: inspection.repository_status,
        })
    }

    pub fn register_project(
        &mut self,
        request: RegisterProjectRequest,
    ) -> Result<ProjectView, ApplicationError> {
        self.inspect_input_before_git(Path::new(request.input_path.trim()))?;
        let inspection = self
            .git
            .inspect_project(Path::new(request.input_path.trim()))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let (root_identity, common_identity) = self.inspect_filesystem_identity(&inspection)?;
        if identity_confirmation_token(
            &inspection.confirmation_token,
            &root_identity,
            common_identity.as_ref(),
        ) != request.confirmation_token
        {
            return Err(conflict());
        }
        let name = request
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&inspection.suggested_name);
        if name.len() > 120 || name.chars().any(char::is_control) {
            return Err(invalid_input());
        }
        let root_path = inspection
            .canonical_root
            .to_str()
            .ok_or_else(invalid_input)?
            .to_owned();
        let now = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let project = ProjectRecord {
            id: ProjectId::new(),
            name: name.to_owned(),
            root_path,
            canonical_path_key: inspection.canonical_key,
            display_path: inspection.display_path,
            created_at_ms: now,
            updated_at_ms: now,
        };
        let identity = ProjectFilesystemIdentityRecord {
            project_id: project.id,
            root_volume_serial_hex: root_identity.volume_serial_hex,
            root_file_id_hex: root_identity.file_id_hex,
            repository_kind: inspection.repository_kind,
            git_common_volume_serial_hex: common_identity
                .as_ref()
                .map(|value| value.volume_serial_hex.clone()),
            git_common_file_id_hex: common_identity
                .as_ref()
                .map(|value| value.file_id_hex.clone()),
            confirmed: true,
            revision: 1,
            verified_at_ms: now,
        };
        self.repository
            .create_project_with_identity(&project, &identity)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(ProjectView::from(ProjectSummary::from(project)))
    }

    pub fn project_status(
        &mut self,
        project_id: ProjectId,
    ) -> Result<ProjectStatusView, ApplicationError> {
        let project = self
            .repository
            .get_project(project_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(not_found)?;
        let inspection = self
            .git
            .inspect_project(Path::new(&project.root_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let expected = self
            .repository
            .get_project_identity(project_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(conflict)?;
        let (actual_root, actual_common) = self.inspect_filesystem_identity(&inspection)?;
        if !expected.confirmed
            || inspection.canonical_key != project.canonical_path_key
            || expected.repository_kind != inspection.repository_kind
            || expected.root_volume_serial_hex != actual_root.volume_serial_hex
            || expected.root_file_id_hex != actual_root.file_id_hex
            || expected.git_common_volume_serial_hex
                != actual_common
                    .as_ref()
                    .map(|value| value.volume_serial_hex.clone())
            || expected.git_common_file_id_hex
                != actual_common
                    .as_ref()
                    .map(|value| value.file_id_hex.clone())
        {
            return Err(conflict());
        }
        Ok(ProjectStatusView {
            project_id,
            repository_kind: inspection.repository_kind,
            repository_status: inspection.repository_status,
        })
    }

    fn inspect_filesystem_identity(
        &mut self,
        inspection: &chatoms_ports::git::ProjectInspection,
    ) -> Result<(DirectoryIdentity, Option<DirectoryIdentity>), ApplicationError> {
        let root = self
            .filesystem
            .inspect_supported_directory(&inspection.canonical_root)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        self.filesystem
            .verify_local_tree(&root.canonical_path)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let common = inspection
            .git_common_dir
            .as_deref()
            .map(|path| self.filesystem.inspect_supported_directory(path))
            .transpose()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok((root, common))
    }

    fn inspect_input_before_git(&mut self, input: &Path) -> Result<(), ApplicationError> {
        let input = self
            .filesystem
            .inspect_supported_directory(input)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        self.filesystem
            .verify_local_tree(&input.canonical_path)
            .map_err(|error| ApplicationError::from_categorized(&error))
    }
}

fn identity_confirmation_token(
    git_token: &str,
    root: &DirectoryIdentity,
    common: Option<&DirectoryIdentity>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(git_token.as_bytes());
    digest.update([0]);
    digest.update(root.volume_serial_hex.as_bytes());
    digest.update([0]);
    digest.update(root.file_id_hex.as_bytes());
    if let Some(common) = common {
        digest.update([0]);
        digest.update(common.volume_serial_hex.as_bytes());
        digest.update([0]);
        digest.update(common.file_id_hex.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid_input() -> ApplicationError {
    category_error(chatoms_ports::error::FailureCategory::InvalidInput)
}

fn conflict() -> ApplicationError {
    category_error(chatoms_ports::error::FailureCategory::Conflict)
}

fn not_found() -> ApplicationError {
    category_error(chatoms_ports::error::FailureCategory::NotFound)
}

fn category_error(category: chatoms_ports::error::FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
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
