use std::{error::Error, fmt};

use chatoms_domain::{ProjectId, Task, TaskId, TaskStateTransition};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryErrorCode {
    ProjectNotFound,
    TaskNotFound,
    DuplicateTask,
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
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveLease {
    pub task_id: TaskId,
    pub acquired_at_ms: i64,
}

pub trait FoundationRepository {
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
}
