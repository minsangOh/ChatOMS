//! Read-only discovery of candidate Testing validation commands, the
//! one-time approval use case that pins one candidate down together with a
//! user-approved executable binding as immutable storage tied to a specific
//! task version, and a read-only re-verification of that binding.
//!
//! Unlike [`crate::planning_execution`] / [`crate::implementation_execution`],
//! this Unit never executes a process and never changes `Task` state:
//! approving a validation command commits nothing but the approval+binding
//! row itself. Running an approved command is a later Unit's
//! responsibility.
//!
//! The executable binding captured here is a deliberately **weaker trust
//! model than Git or Claude/Codex executable trust** — see
//! `docs/DECISIONS.md`'s "Validation tool executable trust" entry and
//! [`chatoms_ports::repository::ValidationCommandApprovalRecord`]'s
//! documentation for why. There is no PATH search (the caller always
//! supplies an absolute path) and no mandatory Authenticode signer gate;
//! trust rests entirely on the user's one-time approval plus Windows stable
//! NTFS object identity, which [`Self::verify_binding`] re-checks on demand
//! and which any future execution Unit must re-check immediately before
//! spawning anything — a mismatch must always require a fresh approval, not
//! an automatic repair.

use std::path::{Path, PathBuf};

use chatoms_domain::{
    ProjectId, TaskId, TaskState, ValidationCommandKind, ValidationExecutionScope,
};
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    filesystem::FilesystemIdentityPort,
    repository::{FoundationRepository, GitIsolationStatus, ValidationCommandApprovalRecord},
    validation::{ValidationCommandCandidate, ValidationCommandDiscovery},
};

use crate::error::ApplicationError;

pub struct ApproveValidationCommandRequest {
    task_id: TaskId,
    expected_version: u64,
    kind: ValidationCommandKind,
    executable: String,
    arguments: Vec<String>,
    approved_executable_path: PathBuf,
    approved_cargo_home_path: Option<PathBuf>,
    approved_rustup_home_path: Option<PathBuf>,
}

impl ApproveValidationCommandRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        kind: ValidationCommandKind,
        executable: String,
        arguments: Vec<String>,
        approved_executable_path: PathBuf,
        approved_cargo_home_path: Option<PathBuf>,
        approved_rustup_home_path: Option<PathBuf>,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            kind,
            executable,
            arguments,
            approved_executable_path,
            approved_cargo_home_path,
            approved_rustup_home_path,
        }
    }
}

pub struct ApproveProjectRootValidationCommandRequest {
    task_id: TaskId,
    expected_version: u64,
    project_id: ProjectId,
    kind: ValidationCommandKind,
    executable: String,
    arguments: Vec<String>,
    approved_executable_path: PathBuf,
    approved_cargo_home_path: Option<PathBuf>,
    approved_rustup_home_path: Option<PathBuf>,
}

impl ApproveProjectRootValidationCommandRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        project_id: ProjectId,
        kind: ValidationCommandKind,
        executable: String,
        arguments: Vec<String>,
        approved_executable_path: PathBuf,
        approved_cargo_home_path: Option<PathBuf>,
        approved_rustup_home_path: Option<PathBuf>,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            project_id,
            kind,
            executable,
            arguments,
            approved_executable_path,
            approved_cargo_home_path,
            approved_rustup_home_path,
        }
    }
}

/// Outcome of read-only re-verifying a stored [`ValidationCommandApprovalRecord`]
/// against the file/directory identity Windows reports right now. Never
/// executes anything; a future execution Unit must treat anything other
/// than `Verified` as "do not spawn — require a fresh approval instead."
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationCommandBindingStatus {
    Verified,
    IdentityMismatch,
    NotFound,
}

pub struct ValidationCommandService<'a, R, T, D, F> {
    repository: &'a mut R,
    time: &'a mut T,
    discovery: &'a mut D,
    filesystem: &'a mut F,
}

impl<'a, R, T, D, F> ValidationCommandService<'a, R, T, D, F>
where
    R: FoundationRepository,
    T: TimeProvider,
    D: ValidationCommandDiscovery,
    F: FilesystemIdentityPort,
{
    #[must_use]
    pub const fn new(
        repository: &'a mut R,
        time: &'a mut T,
        discovery: &'a mut D,
        filesystem: &'a mut F,
    ) -> Self {
        Self {
            repository,
            time,
            discovery,
            filesystem,
        }
    }

    /// Read-only: lists every structured candidate `discovery` proposes for
    /// `task_id`'s own worktree. Never executes anything and never writes.
    pub fn list_candidates(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<Vec<ValidationCommandCandidate>, ApplicationError> {
        let worktree_path = self.load_worktree_path(task_id, expected_version)?;
        self.discovery
            .discover_candidates(Path::new(&worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    /// Approves exactly one discovered candidate for `(task_id,
    /// expected_version, request.kind)` together with a user-approved
    /// executable binding, persisting both as a single immutable row.
    /// Rejects the request unless: the task is `Implementing` or `Testing`
    /// with a matching version; `(request.kind, request.executable,
    /// request.arguments)` exactly matches one of the candidates
    /// `discovery` proposes right now; `request.approved_executable_path`
    /// is absolute and resolves (via [`FilesystemIdentityPort`]) to a
    /// regular, non-reparse file outside the task worktree. Captures the
    /// Windows stable NTFS object identity of that file and its containing
    /// directory at approval time. Never executes the command and never
    /// changes task state.
    pub fn approve_command(
        &mut self,
        request: ApproveValidationCommandRequest,
    ) -> Result<ValidationCommandApprovalRecord, ApplicationError> {
        let worktree_path = self.load_worktree_path(request.task_id, request.expected_version)?;
        let candidates = self
            .discovery
            .discover_candidates(Path::new(&worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let approved = candidates.into_iter().find(|candidate| {
            candidate.kind == request.kind
                && candidate.executable == request.executable
                && candidate.arguments == request.arguments
        });
        if approved.is_none() {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        let binding = self.bind_executable(&worktree_path, &request.approved_executable_path)?;
        let cargo_home = request
            .approved_cargo_home_path
            .as_deref()
            .map(|path| self.bind_environment_directory(&worktree_path, path))
            .transpose()?;
        let rustup_home = request
            .approved_rustup_home_path
            .as_deref()
            .map(|path| self.bind_environment_directory(&worktree_path, path))
            .transpose()?;
        let approved_at_ms = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let approval = ValidationCommandApprovalRecord {
            task_id: request.task_id,
            approved_task_version: request.expected_version,
            execution_scope: ValidationExecutionScope::TaskWorktree,
            kind: request.kind,
            executable: request.executable,
            arguments: request.arguments,
            approved_executable_path: binding.executable_path,
            executable_volume_serial_hex: binding.executable_volume_serial_hex,
            executable_file_id_hex: binding.executable_file_id_hex,
            tool_directory_path: binding.tool_directory_path,
            tool_directory_volume_serial_hex: binding.tool_directory_volume_serial_hex,
            tool_directory_file_id_hex: binding.tool_directory_file_id_hex,
            approved_cargo_home_path: cargo_home.as_ref().map(|home| home.path.clone()),
            cargo_home_volume_serial_hex: cargo_home
                .as_ref()
                .map(|home| home.volume_serial_hex.clone()),
            cargo_home_file_id_hex: cargo_home.as_ref().map(|home| home.file_id_hex.clone()),
            approved_rustup_home_path: rustup_home.as_ref().map(|home| home.path.clone()),
            rustup_home_volume_serial_hex: rustup_home
                .as_ref()
                .map(|home| home.volume_serial_hex.clone()),
            rustup_home_file_id_hex: rustup_home.as_ref().map(|home| home.file_id_hex.clone()),
            target_project_id: None,
            target_project_identity_revision: None,
            target_root_volume_serial_hex: None,
            target_root_file_id_hex: None,
            approved_at_ms,
        };
        self.repository
            .save_validation_command_approval(&approval)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(approval)
    }

    pub fn approve_project_root_command(
        &mut self,
        request: ApproveProjectRootValidationCommandRequest,
    ) -> Result<ValidationCommandApprovalRecord, ApplicationError> {
        let task = self
            .repository
            .get_task(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != request.expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if task.state() != TaskState::AwaitingUserDiffApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        if task.project_id() != request.project_id {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let project = self
            .repository
            .get_project(request.project_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        let project_identity = self
            .repository
            .get_project_identity(request.project_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .filter(|identity| identity.confirmed)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let live_root = self
            .filesystem
            .inspect_supported_directory(Path::new(&project.root_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if live_root.volume_serial_hex != project_identity.root_volume_serial_hex
            || live_root.file_id_hex != project_identity.root_file_id_hex
            || live_root.canonical_path.to_string_lossy() != project.root_path
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let isolation = self
            .repository
            .get_task_isolation(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if isolation.project_id != task.project_id()
            || isolation.expected_task_version != request.expected_version
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let worktree_path = isolation
            .worktree_path
            .filter(|_| isolation.status == GitIsolationStatus::WorktreeReady)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let candidates = self
            .discovery
            .discover_candidates(Path::new(&worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let fixed_cargo = request.executable == "cargo"
            && expected_cargo_arguments(request.kind).is_some_and(|arguments| {
                request.arguments.iter().map(String::as_str).eq(arguments)
            });
        if !fixed_cargo
            || !candidates.iter().any(|candidate| {
                candidate.kind == request.kind
                    && candidate.executable == request.executable
                    && candidate.arguments == request.arguments
            })
        {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        let binding =
            self.bind_executable(&project.root_path, &request.approved_executable_path)?;
        let cargo_home = request
            .approved_cargo_home_path
            .as_deref()
            .map(|path| self.bind_environment_directory(&project.root_path, path))
            .transpose()?;
        let rustup_home = request
            .approved_rustup_home_path
            .as_deref()
            .map(|path| self.bind_environment_directory(&project.root_path, path))
            .transpose()?;
        let approved_at_ms = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let approval = ValidationCommandApprovalRecord {
            task_id: request.task_id,
            approved_task_version: request.expected_version,
            execution_scope: ValidationExecutionScope::ProjectRoot,
            kind: request.kind,
            executable: request.executable,
            arguments: request.arguments,
            approved_executable_path: binding.executable_path,
            executable_volume_serial_hex: binding.executable_volume_serial_hex,
            executable_file_id_hex: binding.executable_file_id_hex,
            tool_directory_path: binding.tool_directory_path,
            tool_directory_volume_serial_hex: binding.tool_directory_volume_serial_hex,
            tool_directory_file_id_hex: binding.tool_directory_file_id_hex,
            approved_cargo_home_path: cargo_home.as_ref().map(|home| home.path.clone()),
            cargo_home_volume_serial_hex: cargo_home
                .as_ref()
                .map(|home| home.volume_serial_hex.clone()),
            cargo_home_file_id_hex: cargo_home.as_ref().map(|home| home.file_id_hex.clone()),
            approved_rustup_home_path: rustup_home.as_ref().map(|home| home.path.clone()),
            rustup_home_volume_serial_hex: rustup_home
                .as_ref()
                .map(|home| home.volume_serial_hex.clone()),
            rustup_home_file_id_hex: rustup_home.as_ref().map(|home| home.file_id_hex.clone()),
            target_project_id: Some(request.project_id),
            target_project_identity_revision: Some(project_identity.revision),
            target_root_volume_serial_hex: Some(project_identity.root_volume_serial_hex),
            target_root_file_id_hex: Some(project_identity.root_file_id_hex),
            approved_at_ms,
        };
        self.repository
            .save_validation_command_approval(&approval)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(approval)
    }

    /// Read-only: re-verifies a previously stored binding for `(task_id,
    /// approved_task_version, kind)` against the file/directory identity
    /// Windows reports right now. Never executes anything, never writes,
    /// and never repairs a mismatch — the caller must treat anything other
    /// than `Verified` as "require a fresh approval."
    pub fn verify_binding(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<ValidationCommandBindingStatus, ApplicationError> {
        let approvals = self
            .repository
            .list_validation_command_approvals(task_id, approved_task_version)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let Some(approval) = approvals.into_iter().find(|approval| approval.kind == kind) else {
            return Ok(ValidationCommandBindingStatus::NotFound);
        };
        let Ok(current_executable) = self
            .filesystem
            .inspect_supported_file(Path::new(&approval.approved_executable_path))
        else {
            return Ok(ValidationCommandBindingStatus::IdentityMismatch);
        };
        let Ok(current_tool_directory) = self
            .filesystem
            .inspect_supported_directory(Path::new(&approval.tool_directory_path))
        else {
            return Ok(ValidationCommandBindingStatus::IdentityMismatch);
        };
        let matches = current_executable.volume_serial_hex == approval.executable_volume_serial_hex
            && current_executable.file_id_hex == approval.executable_file_id_hex
            && current_executable.canonical_path.to_string_lossy()
                == approval.approved_executable_path
            && current_tool_directory.volume_serial_hex
                == approval.tool_directory_volume_serial_hex
            && current_tool_directory.file_id_hex == approval.tool_directory_file_id_hex;
        if !matches {
            return Ok(ValidationCommandBindingStatus::IdentityMismatch);
        }
        let cargo_home_matches = self.environment_binding_matches(
            &approval.approved_cargo_home_path,
            &approval.cargo_home_volume_serial_hex,
            &approval.cargo_home_file_id_hex,
        );
        let rustup_home_matches = self.environment_binding_matches(
            &approval.approved_rustup_home_path,
            &approval.rustup_home_volume_serial_hex,
            &approval.rustup_home_file_id_hex,
        );
        Ok(if cargo_home_matches && rustup_home_matches {
            ValidationCommandBindingStatus::Verified
        } else {
            ValidationCommandBindingStatus::IdentityMismatch
        })
    }

    /// Re-verifies an optional `CARGO_HOME`/`RUSTUP_HOME` trio from
    /// [`verify_binding`](Self::verify_binding). `None` (no approved
    /// override) always matches; a stored `Some` trio must still resolve to
    /// the exact same canonical path and stable identity, and any
    /// inspection failure (including a path that can no longer be
    /// inspected) is treated as a mismatch, never as "no override approved."
    fn environment_binding_matches(
        &mut self,
        approved_path: &Option<String>,
        approved_volume_serial_hex: &Option<String>,
        approved_file_id_hex: &Option<String>,
    ) -> bool {
        let (Some(path), Some(volume_serial_hex), Some(file_id_hex)) = (
            approved_path.as_deref(),
            approved_volume_serial_hex.as_deref(),
            approved_file_id_hex.as_deref(),
        ) else {
            return approved_path.is_none()
                && approved_volume_serial_hex.is_none()
                && approved_file_id_hex.is_none();
        };
        let Ok(current) = self.filesystem.inspect_supported_directory(Path::new(path)) else {
            return false;
        };
        current.volume_serial_hex == volume_serial_hex
            && current.file_id_hex == file_id_hex
            && current.canonical_path.to_string_lossy() == path
    }

    /// Verifies `approved_executable_path` from scratch: must be absolute,
    /// must resolve to a regular non-reparse file (via
    /// [`FilesystemIdentityPort::inspect_supported_file`]) outside
    /// `worktree_path`, and its containing directory's identity is captured
    /// too (the future controlled-PATH value). No PATH search, no signer
    /// check — path plus stable file identity is the entire trust basis
    /// here (see this module's docs for why that is weaker than Git/Claude
    /// executable trust).
    fn bind_executable(
        &mut self,
        worktree_path: &str,
        approved_executable_path: &Path,
    ) -> Result<ExecutableBinding, ApplicationError> {
        if !approved_executable_path.is_absolute() {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        let executable_identity = self
            .filesystem
            .inspect_supported_file(approved_executable_path)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let worktree_identity = self
            .filesystem
            .inspect_supported_directory(Path::new(worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if executable_identity
            .canonical_path
            .starts_with(&worktree_identity.canonical_path)
        {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        let tool_directory = executable_identity
            .canonical_path
            .parent()
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let tool_directory_identity = self
            .filesystem
            .inspect_supported_directory(tool_directory)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(ExecutableBinding {
            executable_path: executable_identity
                .canonical_path
                .to_string_lossy()
                .into_owned(),
            executable_volume_serial_hex: executable_identity.volume_serial_hex,
            executable_file_id_hex: executable_identity.file_id_hex,
            tool_directory_path: tool_directory_identity
                .canonical_path
                .to_string_lossy()
                .into_owned(),
            tool_directory_volume_serial_hex: tool_directory_identity.volume_serial_hex,
            tool_directory_file_id_hex: tool_directory_identity.file_id_hex,
        })
    }

    /// Verifies a user-supplied `CARGO_HOME`/`RUSTUP_HOME` override from
    /// scratch, exactly like [`Self::bind_executable`]'s worktree-escape
    /// check: must be absolute and must resolve (via
    /// [`FilesystemIdentityPort::inspect_supported_directory`]) to a
    /// directory outside `worktree_path`. No PATH search — path plus stable
    /// directory identity is the entire trust basis, captured here so a
    /// future executor re-verifies against this durable record instead of a
    /// value it was separately constructed with.
    fn bind_environment_directory(
        &mut self,
        worktree_path: &str,
        approved_directory_path: &Path,
    ) -> Result<EnvironmentDirectoryBinding, ApplicationError> {
        if !approved_directory_path.is_absolute() {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        let directory_identity = self
            .filesystem
            .inspect_supported_directory(approved_directory_path)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let worktree_identity = self
            .filesystem
            .inspect_supported_directory(Path::new(worktree_path))
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        if directory_identity
            .canonical_path
            .starts_with(&worktree_identity.canonical_path)
        {
            return Err(category_error(FailureCategory::InvalidInput));
        }
        Ok(EnvironmentDirectoryBinding {
            path: directory_identity
                .canonical_path
                .to_string_lossy()
                .into_owned(),
            volume_serial_hex: directory_identity.volume_serial_hex,
            file_id_hex: directory_identity.file_id_hex,
        })
    }

    /// Loads `task_id`, verifies its version and that it is `Implementing`
    /// or `Testing` (the only states this Unit's discovery/approval flow may
    /// run in), then resolves its `WorktreeReady` worktree path. Read-only.
    fn load_worktree_path(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<String, ApplicationError> {
        let task = self
            .repository
            .get_task(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        if !matches!(task.state(), TaskState::Implementing | TaskState::Testing) {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let isolation = self
            .repository
            .get_task_isolation(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        isolation
            .worktree_path
            .filter(|_| isolation.status == GitIsolationStatus::WorktreeReady)
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))
    }
}

struct ExecutableBinding {
    executable_path: String,
    executable_volume_serial_hex: String,
    executable_file_id_hex: String,
    tool_directory_path: String,
    tool_directory_volume_serial_hex: String,
    tool_directory_file_id_hex: String,
}

struct EnvironmentDirectoryBinding {
    path: String,
    volume_serial_hex: String,
    file_id_hex: String,
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}

fn expected_cargo_arguments(
    kind: ValidationCommandKind,
) -> Option<impl Iterator<Item = &'static str>> {
    let arguments: &'static [&'static str] = match kind {
        ValidationCommandKind::Format => &["fmt", "--all", "--", "--check"],
        ValidationCommandKind::Lint => {
            &["clippy", "--workspace", "--all-targets", "--all-features"]
        }
        ValidationCommandKind::Typecheck => return None,
        ValidationCommandKind::Test => &["test", "--workspace"],
        ValidationCommandKind::Build => &["build", "--workspace"],
    };
    Some(arguments.iter().copied())
}
