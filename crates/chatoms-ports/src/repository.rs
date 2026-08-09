use std::{error::Error, fmt};

use chatoms_domain::{GitOperationId, ProjectId, Task, TaskId, TaskStateTransition};

use crate::git::RepositoryKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryErrorCode {
    ProjectNotFound,
    DuplicateProject,
    TaskNotFound,
    DuplicateTask,
    IsolationNotFound,
    DuplicateIsolation,
    VersionConflict,
    TransitionSequenceConflict,
    ActiveLeaseConflict,
    InvalidAggregate,
    InvalidPersistenceState,
    DatabaseUnavailable,
    OperationFailed,
}

#[derive(Debug)]
pub struct RepositoryError {
    code: RepositoryErrorCode,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl RepositoryError {
    #[must_use]
    pub const fn new(code: RepositoryErrorCode) -> Self {
        Self { code, source: None }
    }

    #[must_use]
    pub fn with_source(
        code: RepositoryErrorCode,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            source: Some(Box::new(source)),
        }
    }

    #[must_use]
    pub const fn code(&self) -> RepositoryErrorCode {
        self.code
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "repository operation failed: {:?}", self.code)
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub name: String,
    pub root_path: String,
    pub canonical_path_key: String,
    pub display_path: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub name: String,
    pub root_path: String,
    pub canonical_path_key: String,
    pub display_path: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectFilesystemIdentityRecord {
    pub project_id: ProjectId,
    pub root_volume_serial_hex: String,
    pub root_file_id_hex: String,
    pub repository_kind: RepositoryKind,
    pub git_common_volume_serial_hex: Option<String>,
    pub git_common_file_id_hex: Option<String>,
    pub confirmed: bool,
    pub revision: u64,
    pub verified_at_ms: i64,
}

impl From<ProjectRecord> for ProjectSummary {
    fn from(value: ProjectRecord) -> Self {
        Self {
            id: value.id,
            name: value.name,
            root_path: value.root_path,
            canonical_path_key: value.canonical_path_key,
            display_path: value.display_path,
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitIsolationStatus {
    AwaitingGitInitApproval,
    Ready,
    GitInitInProgress,
    WorktreeCreating,
    WorktreeReady,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskGitIsolation {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub status: GitIsolationStatus,
    pub operation_id: Option<GitOperationId>,
    pub expected_task_version: u64,
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
    pub worktree_path: Option<String>,
    pub branch_created_by_app: bool,
    pub worktree_created_by_app: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitInitApproval {
    pub operation_id: GitOperationId,
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub approved_task_version: u64,
    pub approved_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitOperationKind {
    GitInitialize,
    WorktreeCreate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitOperationReceiptKind {
    CommandStarted,
    CommandSucceeded,
    PostVerified,
    CompletionRecorded,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitOperationAttemptStatus {
    IntentRecorded,
    RecoveryRequired,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitOperationAttempt {
    pub operation_id: GitOperationId,
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub operation_kind: GitOperationKind,
    pub status: GitOperationAttemptStatus,
    pub approved_task_version: u64,
    pub project_identity_revision: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitOperationReceipt {
    pub operation_id: GitOperationId,
    pub sequence: u64,
    pub kind: GitOperationReceiptKind,
    pub evidence: Option<String>,
    pub recorded_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveLease {
    pub task_id: TaskId,
    pub acquired_at_ms: i64,
}

pub trait FoundationRepository {
    fn create_project(&mut self, _project: &ProjectRecord) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn create_project_with_identity(
        &mut self,
        _project: &ProjectRecord,
        _identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_project_identity(
        &mut self,
        _project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn update_project_identity(
        &mut self,
        _identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_project(
        &mut self,
        _project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError>;

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError>;

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError>;

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError>;

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError>;

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError>;

    fn next_transition_sequence(&mut self, task_id: TaskId) -> Result<u64, RepositoryError> {
        let transitions = self.list_task_transitions(task_id)?;
        let previous = transitions
            .last()
            .map(TaskStateTransition::sequence)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::InvalidPersistenceState))?;
        TaskStateTransition::checked_next_sequence(previous)
            .map_err(|_| RepositoryError::new(RepositoryErrorCode::TransitionSequenceConflict))
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError>;

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError>;

    fn create_isolation_task(
        &mut self,
        _task: &Task,
        _initial_transition: &TaskStateTransition,
        _classified_transition: &TaskStateTransition,
        _lease_acquired_at_ms: i64,
        _isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_task_isolation(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn begin_git_initialization(
        &mut self,
        _expected_version: u64,
        _isolation: &TaskGitIsolation,
        _approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn save_isolation_intent(
        &mut self,
        _expected_version: u64,
        _isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn append_git_operation_receipt(
        &mut self,
        _operation_id: GitOperationId,
        _kind: GitOperationReceiptKind,
        _evidence: Option<&str>,
        _recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn list_git_operation_receipts(
        &mut self,
        _operation_id: GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn save_isolation_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn save_git_initialization_completion(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _isolation: &TaskGitIsolation,
        _identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn save_worktree_completion(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn terminate_isolation_task(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }
}
