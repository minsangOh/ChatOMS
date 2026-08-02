use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{DomainError, ProjectId, TaskId, TaskState};

const TASK_BRANCH_PREFIX: &str = "ai-task/";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TaskBranchIdentity(String);

impl TaskBranchIdentity {
    #[must_use]
    pub fn for_task(task_id: TaskId) -> Self {
        Self(format!("{TASK_BRANCH_PREFIX}{task_id}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn belongs_to(&self, task_id: TaskId) -> bool {
        self == &Self::for_task(task_id)
    }
}

impl fmt::Display for TaskBranchIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for TaskBranchIdentity {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let suffix = value
            .strip_prefix(TASK_BRANCH_PREFIX)
            .ok_or(DomainError::InvalidTaskBranchIdentity)?;
        if suffix.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(DomainError::InvalidTaskBranchIdentity);
        }

        let task_id = suffix
            .parse::<TaskId>()
            .map_err(|_| DomainError::InvalidTaskBranchIdentity)?;
        if task_id.to_string() != suffix {
            return Err(DomainError::InvalidTaskBranchIdentity);
        }

        Ok(Self(value.to_owned()))
    }
}

impl Serialize for TaskBranchIdentity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TaskBranchIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResumeValidation(());

impl ResumeValidation {
    #[must_use]
    pub const fn from_completed_checks() -> Self {
        Self(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryValidation(());

impl RecoveryValidation {
    #[must_use]
    pub const fn from_completed_checks() -> Self {
        Self(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub state: TaskState,
    pub version: u64,
    pub task_branch_identity: TaskBranchIdentity,
    pub resume_target_state: Option<TaskState>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Task {
    id: TaskId,
    project_id: ProjectId,
    state: TaskState,
    version: u64,
    task_branch_identity: TaskBranchIdentity,
    resume_target_state: Option<TaskState>,
    created_at_ms: i64,
    updated_at_ms: i64,
    terminal_at_ms: Option<i64>,
}

impl Task {
    #[must_use]
    pub fn new(id: TaskId, project_id: ProjectId, created_at_ms: i64) -> Self {
        Self {
            id,
            project_id,
            state: TaskState::Created,
            version: 0,
            task_branch_identity: TaskBranchIdentity::for_task(id),
            resume_target_state: None,
            created_at_ms,
            updated_at_ms: created_at_ms,
            terminal_at_ms: None,
        }
    }

    pub fn restore(snapshot: TaskSnapshot) -> Result<Self, DomainError> {
        let task = Self {
            id: snapshot.id,
            project_id: snapshot.project_id,
            state: snapshot.state,
            version: snapshot.version,
            task_branch_identity: snapshot.task_branch_identity,
            resume_target_state: snapshot.resume_target_state,
            created_at_ms: snapshot.created_at_ms,
            updated_at_ms: snapshot.updated_at_ms,
            terminal_at_ms: snapshot.terminal_at_ms,
        };
        task.validate_invariants()?;
        Ok(task)
    }

    #[must_use]
    pub fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            id: self.id,
            project_id: self.project_id,
            state: self.state,
            version: self.version,
            task_branch_identity: self.task_branch_identity.clone(),
            resume_target_state: self.resume_target_state,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            terminal_at_ms: self.terminal_at_ms,
        }
    }

    pub fn validate_invariants(&self) -> Result<(), DomainError> {
        if !self.task_branch_identity.belongs_to(self.id) {
            return Err(DomainError::InvariantViolation);
        }
        if self.updated_at_ms < self.created_at_ms {
            return Err(DomainError::InvalidTimestamp);
        }
        if matches!(self.state, TaskState::Created) && self.version != 0 {
            return Err(DomainError::InvalidVersion);
        }
        if !matches!(self.state, TaskState::Created) && self.version == 0 {
            return Err(DomainError::InvalidVersion);
        }

        match self.state {
            TaskState::Paused => {
                if !self
                    .resume_target_state
                    .is_some_and(TaskState::is_resume_target)
                {
                    return Err(DomainError::InvariantViolation);
                }
            }
            TaskState::RecoveryRequired => {
                if self
                    .resume_target_state
                    .is_some_and(|target| !target.is_resume_target())
                {
                    return Err(DomainError::InvariantViolation);
                }
            }
            _ => {
                if self.resume_target_state.is_some() {
                    return Err(DomainError::InvariantViolation);
                }
            }
        }

        if self.state.is_terminal() || self.state.is_post_terminal() {
            let terminal_at_ms = self.terminal_at_ms.ok_or(DomainError::InvariantViolation)?;
            if terminal_at_ms < self.created_at_ms || terminal_at_ms > self.updated_at_ms {
                return Err(DomainError::InvalidTimestamp);
            }
        } else if self.terminal_at_ms.is_some() {
            return Err(DomainError::InvariantViolation);
        }

        Ok(())
    }

    pub fn transition_to(
        &mut self,
        next: TaskState,
        occurred_at_ms: i64,
    ) -> Result<(), DomainError> {
        self.state.validate_transition(next)?;
        let next_version = self.next_version()?;
        self.validate_transition_time(occurred_at_ms)?;

        self.state = next;
        self.version = next_version;
        self.updated_at_ms = occurred_at_ms;
        if next.is_terminal() {
            self.terminal_at_ms = Some(occurred_at_ms);
        }
        if matches!(next, TaskState::RecoveryRequired)
            || (!matches!(next, TaskState::Paused | TaskState::RecoveryRequired))
        {
            self.resume_target_state = None;
        }
        self.validate_invariants()
    }

    pub fn pause(&mut self, occurred_at_ms: i64) -> Result<(), DomainError> {
        if !self.state.can_pause() {
            return Err(DomainError::InvalidStateTransition);
        }
        let next_version = self.next_version()?;
        self.validate_transition_time(occurred_at_ms)?;

        let resume_target = self.state;
        self.state = TaskState::Paused;
        self.version = next_version;
        self.updated_at_ms = occurred_at_ms;
        self.resume_target_state = Some(resume_target);
        self.validate_invariants()
    }

    pub fn resume_from_pause(
        &mut self,
        expected_target: TaskState,
        _validation: ResumeValidation,
        occurred_at_ms: i64,
    ) -> Result<(), DomainError> {
        if !matches!(self.state, TaskState::Paused)
            || !expected_target.is_resume_target()
            || self.resume_target_state != Some(expected_target)
        {
            return Err(DomainError::InvalidStateTransition);
        }
        let next_version = self.next_version()?;
        self.validate_transition_time(occurred_at_ms)?;

        self.state = expected_target;
        self.version = next_version;
        self.updated_at_ms = occurred_at_ms;
        self.resume_target_state = None;
        self.validate_invariants()
    }

    pub fn set_recovery_target(
        &mut self,
        target: TaskState,
        _validation: RecoveryValidation,
    ) -> Result<(), DomainError> {
        if !matches!(self.state, TaskState::RecoveryRequired) || !target.is_resume_target() {
            return Err(DomainError::InvalidStateTransition);
        }
        self.resume_target_state = Some(target);
        self.validate_invariants()
    }

    pub fn pause_from_recovery(
        &mut self,
        _validation: RecoveryValidation,
        occurred_at_ms: i64,
    ) -> Result<(), DomainError> {
        let target = self.resume_target_state;
        if !matches!(self.state, TaskState::RecoveryRequired)
            || !target.is_some_and(TaskState::is_resume_target)
        {
            return Err(DomainError::InvalidStateTransition);
        }
        let next_version = self.next_version()?;
        self.validate_transition_time(occurred_at_ms)?;

        self.state = TaskState::Paused;
        self.version = next_version;
        self.updated_at_ms = occurred_at_ms;
        self.validate_invariants()
    }

    pub fn resume_from_recovery(
        &mut self,
        expected_target: TaskState,
        _validation: RecoveryValidation,
        occurred_at_ms: i64,
    ) -> Result<(), DomainError> {
        if !matches!(self.state, TaskState::RecoveryRequired)
            || !expected_target.is_resume_target()
            || self.resume_target_state != Some(expected_target)
        {
            return Err(DomainError::InvalidStateTransition);
        }
        let next_version = self.next_version()?;
        self.validate_transition_time(occurred_at_ms)?;

        self.state = expected_target;
        self.version = next_version;
        self.updated_at_ms = occurred_at_ms;
        self.resume_target_state = None;
        self.validate_invariants()
    }

    fn next_version(&self) -> Result<u64, DomainError> {
        self.version
            .checked_add(1)
            .ok_or(DomainError::InvalidVersion)
    }

    fn validate_transition_time(&self, occurred_at_ms: i64) -> Result<(), DomainError> {
        if occurred_at_ms < self.updated_at_ms {
            Err(DomainError::InvalidTimestamp)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub const fn id(&self) -> TaskId {
        self.id
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    #[must_use]
    pub const fn state(&self) -> TaskState {
        self.state
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub fn task_branch_identity(&self) -> &TaskBranchIdentity {
        &self.task_branch_identity
    }

    #[must_use]
    pub const fn resume_target_state(&self) -> Option<TaskState> {
        self.resume_target_state
    }

    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    #[must_use]
    pub const fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }

    #[must_use]
    pub const fn terminal_at_ms(&self) -> Option<i64> {
        self.terminal_at_ms
    }
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = TaskSnapshot::deserialize(deserializer)?;
        Self::restore(snapshot).map_err(D::Error::custom)
    }
}
