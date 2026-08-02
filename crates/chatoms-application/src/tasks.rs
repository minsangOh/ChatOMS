use std::str::FromStr;

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId, TaskState,
    TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot,
};
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    repository::{ActiveLease, FoundationRepository},
};

use crate::error::ApplicationError;

pub struct CreateTaskRequest {
    project_id: ProjectId,
    actor_kind: String,
    reason_code: String,
}

impl CreateTaskRequest {
    #[must_use]
    pub fn new(project_id: ProjectId, actor_kind: String, reason_code: String) -> Self {
        Self {
            project_id,
            actor_kind,
            reason_code,
        }
    }
}

pub struct TransitionTaskRequest {
    task_id: TaskId,
    expected_version: u64,
    target_state: TaskState,
    actor_kind: String,
    reason_code: String,
}

impl TransitionTaskRequest {
    #[must_use]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        target_state: TaskState,
        actor_kind: String,
        reason_code: String,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            target_state,
            actor_kind,
            reason_code,
        }
    }
}

pub struct TaskActionRequest {
    task_id: TaskId,
    expected_version: u64,
    actor_kind: String,
    reason_code: String,
}

impl TaskActionRequest {
    #[must_use]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        actor_kind: String,
        reason_code: String,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            actor_kind,
            reason_code,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskView {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub state: TaskState,
    pub version: u64,
    pub branch_identity: TaskBranchIdentity,
    pub resume_target_state: Option<TaskState>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}

impl From<&Task> for TaskView {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id(),
            project_id: task.project_id(),
            state: task.state(),
            version: task.version(),
            branch_identity: task.task_branch_identity().clone(),
            resume_target_state: task.resume_target_state(),
            created_at_ms: task.created_at_ms(),
            updated_at_ms: task.updated_at_ms(),
            terminal_at_ms: task.terminal_at_ms(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveTaskView {
    pub task_id: TaskId,
    pub acquired_at_ms: i64,
}

impl From<ActiveLease> for ActiveTaskView {
    fn from(value: ActiveLease) -> Self {
        Self {
            task_id: value.task_id,
            acquired_at_ms: value.acquired_at_ms,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskTransitionView {
    pub sequence: u64,
    pub from_state: Option<TaskState>,
    pub to_state: TaskState,
    pub task_version: u64,
    pub occurred_at_ms: i64,
}

impl From<TaskStateTransition> for TaskTransitionView {
    fn from(value: TaskStateTransition) -> Self {
        Self {
            sequence: value.sequence(),
            from_state: value.from_state(),
            to_state: value.to_state(),
            task_version: value.task_version(),
            occurred_at_ms: value.occurred_at_ms(),
        }
    }
}

pub struct TaskService<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> TaskService<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    pub fn create_task(
        &mut self,
        request: CreateTaskRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let occurred_at_ms = self.now_ms()?;
        let task_id = TaskId::new();
        let task = Task::new(task_id, request.project_id, occurred_at_ms);
        let transition = TaskStateTransition::initial(
            TaskStateTransitionId::new(),
            task_id,
            actor_kind,
            reason_code,
            occurred_at_ms,
        );
        self.repository
            .create_task(&task, &transition, occurred_at_ms)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    pub fn get_active_task(&mut self) -> Result<Option<ActiveTaskView>, ApplicationError> {
        self.repository
            .active_lease()
            .map(|lease| lease.map(ActiveTaskView::from))
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn get_task(&mut self, task_id: TaskId) -> Result<Option<TaskView>, ApplicationError> {
        self.repository
            .get_task(task_id)
            .map(|task| task.as_ref().map(TaskView::from))
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn task_history(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskTransitionView>, ApplicationError> {
        self.repository
            .list_task_transitions(task_id)
            .map(|transitions| {
                transitions
                    .into_iter()
                    .map(TaskTransitionView::from)
                    .collect()
            })
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn transition_task(
        &mut self,
        request: TransitionTaskRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        self.apply_static_transition(
            request.task_id,
            request.expected_version,
            request.target_state,
            actor_kind,
            reason_code,
            false,
        )
    }

    pub fn pause_task(&mut self, request: TaskActionRequest) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        task.pause(occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        self.repository
            .save_transition(request.expected_version, &task, &transition)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    pub fn mark_recovery_required(
        &mut self,
        request: TaskActionRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        self.apply_static_transition(
            request.task_id,
            request.expected_version,
            TaskState::RecoveryRequired,
            actor_kind,
            reason_code,
            false,
        )
    }

    pub fn complete_task(
        &mut self,
        request: TaskActionRequest,
    ) -> Result<TaskView, ApplicationError> {
        self.terminal_transition(request, TaskState::Completed)
    }

    pub fn fail_task(&mut self, request: TaskActionRequest) -> Result<TaskView, ApplicationError> {
        self.terminal_transition(request, TaskState::Failed)
    }

    pub fn cancel_task(
        &mut self,
        request: TaskActionRequest,
    ) -> Result<TaskView, ApplicationError> {
        self.terminal_transition(request, TaskState::Cancelled)
    }

    pub fn resume_paused_task(&mut self, _task_id: TaskId) -> Result<TaskView, ApplicationError> {
        Err(unsupported_validation())
    }

    pub fn set_recovery_target(
        &mut self,
        _task_id: TaskId,
        _target: TaskState,
    ) -> Result<TaskView, ApplicationError> {
        Err(unsupported_validation())
    }

    pub fn resume_recovered_task(
        &mut self,
        _task_id: TaskId,
    ) -> Result<TaskView, ApplicationError> {
        Err(unsupported_validation())
    }

    pub fn pause_recovery_task(&mut self, _task_id: TaskId) -> Result<TaskView, ApplicationError> {
        Err(unsupported_validation())
    }

    fn terminal_transition(
        &mut self,
        request: TaskActionRequest,
        target: TaskState,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        self.apply_static_transition(
            request.task_id,
            request.expected_version,
            target,
            actor_kind,
            reason_code,
            true,
        )
    }

    fn apply_static_transition(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        target: TaskState,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
        terminal: bool,
    ) -> Result<TaskView, ApplicationError> {
        let mut task = self.load_expected_task(task_id, expected_version)?;
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        task.transition_to(target, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        let result = if terminal {
            self.repository
                .terminate_task(expected_version, &task, &transition)
        } else {
            self.repository
                .save_transition(expected_version, &task, &transition)
        };
        result.map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    fn load_expected_task(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<Task, ApplicationError> {
        let task = self
            .repository
            .get_task(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if task.version() != expected_version {
            return Err(category_error(FailureCategory::VersionConflict));
        }
        Ok(task)
    }

    fn next_transition(
        &mut self,
        task: &Task,
        from_state: TaskState,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskStateTransition, ApplicationError> {
        let sequence = self
            .repository
            .next_transition_sequence(task.id())
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        TaskStateTransition::new(TaskStateTransitionSnapshot {
            id: TaskStateTransitionId::new(),
            task_id: task.id(),
            sequence,
            from_state: Some(from_state),
            to_state: task.state(),
            task_version: task.version(),
            actor_kind,
            reason_code,
            occurred_at_ms: task.updated_at_ms(),
        })
        .map_err(|error| ApplicationError::from_domain(&error))
    }

    fn now_ms(&mut self) -> Result<i64, ApplicationError> {
        self.time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))
    }
}

fn parse_actor(value: &str) -> Result<ActorKind, ApplicationError> {
    ActorKind::from_str(value).map_err(|error| ApplicationError::from_domain(&error))
}

fn parse_reason(value: &str) -> Result<ReasonCode, ApplicationError> {
    ReasonCode::from_str(value).map_err(|error| ApplicationError::from_domain(&error))
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}

fn unsupported_validation() -> ApplicationError {
    category_error(FailureCategory::Unsupported)
}
