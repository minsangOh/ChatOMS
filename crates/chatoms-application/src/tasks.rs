use std::str::FromStr;

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId, TaskState,
    TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot, WorkKind,
};
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    provider::ProviderKind,
    repository::{
        ActiveLease, FoundationRepository, GitIsolationStatus, PlanningResultOutcome,
        ProviderConsent, TaskBriefRecord, TaskPlanningResultRecord,
    },
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

/// Starts Claude Planning from `WorktreeReady`. `actor_kind`/`reason_code`
/// describe who approved the provider-transmission consent and why.
pub struct StartPlanningRequest {
    task_id: TaskId,
    expected_version: u64,
    actor_kind: String,
    reason_code: String,
}

impl StartPlanningRequest {
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

/// Records a Claude Planning attempt's already-safe outcome (parsed,
/// masked, and size-capped by the infrastructure adapter — this request
/// never carries raw provider output) and, on `Completed`, drives the
/// `Planning -> AwaitingDesignApproval` transition atomically with it.
/// `started_at_ms` is caller-supplied because only the caller (the future
/// orchestrator that invoked the adapter) observed when the attempt began;
/// `completed_at_ms` is sourced from this service's own `TimeProvider`.
pub struct RecordPlanningResultRequest {
    task_id: TaskId,
    expected_version: u64,
    outcome: PlanningResultOutcome,
    exit_code: Option<i32>,
    turn_count: Option<u32>,
    plan_text: Option<String>,
    started_at_ms: i64,
    actor_kind: String,
    reason_code: String,
}

impl RecordPlanningResultRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        outcome: PlanningResultOutcome,
        exit_code: Option<i32>,
        turn_count: Option<u32>,
        plan_text: Option<String>,
        started_at_ms: i64,
        actor_kind: String,
        reason_code: String,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            outcome,
            exit_code,
            turn_count,
            plan_text,
            started_at_ms,
            actor_kind,
            reason_code,
        }
    }
}

/// Read-only view of an already-safe, immutable Claude Planning result
/// (see [`TaskPlanningResultRecord`] for the masking/size-bound guarantees
/// this is a direct passthrough of). Never carries raw provider output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningResultView {
    pub outcome: PlanningResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub plan_text: Option<String>,
}

impl From<TaskPlanningResultRecord> for PlanningResultView {
    fn from(value: TaskPlanningResultRecord) -> Self {
        Self {
            outcome: value.outcome,
            exit_code: value.exit_code,
            turn_count: value.turn_count,
            started_at_ms: value.started_at_ms,
            completed_at_ms: value.completed_at_ms,
            plan_text: value.plan_text,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskBriefView {
    pub requirements: String,
    pub completion_criteria: String,
    pub prohibited_scope: String,
    pub created_at_ms: i64,
}

impl From<TaskBriefRecord> for TaskBriefView {
    fn from(value: TaskBriefRecord) -> Self {
        Self {
            requirements: value.requirements,
            completion_criteria: value.completion_criteria,
            prohibited_scope: value.prohibited_scope,
            created_at_ms: value.created_at_ms,
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
    pub brief: Option<TaskBriefView>,
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
            brief: None,
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
        let task = self
            .repository
            .get_task(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let Some(task) = task else {
            return Ok(None);
        };
        let brief = self
            .repository
            .get_task_brief(task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let mut view = TaskView::from(&task);
        view.brief = brief.map(TaskBriefView::from);
        Ok(Some(view))
    }

    /// Reads back the immutable Claude Planning result for `task_id`, if any
    /// attempt has been recorded. A pure passthrough of the already-safe
    /// stored record — callers that only want to expose this in
    /// `AwaitingDesignApproval` must apply that gating themselves (see
    /// `src-tauri/src/commands/planning.rs`).
    pub fn get_planning_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<PlanningResultView>, ApplicationError> {
        self.repository
            .get_task_planning_result(task_id)
            .map(|result| result.map(PlanningResultView::from))
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

    /// Starts Claude Planning: verifies the task is `WorktreeReady` with a
    /// verified-ready isolation record, reuses a valid same-version consent
    /// if one already exists (otherwise records a new one), and commits the
    /// consent (when new), the `Planning` state update, and the transition
    /// history entry in a single repository transaction.
    pub fn start_planning(
        &mut self,
        request: StartPlanningRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::WorktreeReady {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let isolation = self
            .repository
            .get_task_isolation(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let existing_consent = self
            .repository
            .get_provider_consent(
                request.task_id,
                ProviderKind::Claude,
                WorkKind::Planning,
                request.expected_version,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let occurred_at_ms = self.now_ms()?;
        let new_consent = existing_consent.is_none().then_some(ProviderConsent {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: request.expected_version,
            consented_at_ms: occurred_at_ms,
        });
        let from_state = task.state();
        task.transition_to(TaskState::Planning, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        self.repository
            .save_planning_transition(
                request.expected_version,
                &task,
                &transition,
                new_consent.as_ref(),
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    /// Records a Claude Planning attempt's already-safe outcome and, in the
    /// same repository transaction, drives the resulting state transition
    /// (`Completed -> AwaitingDesignApproval`, `Failed -> Failed`,
    /// `RecoveryRequired -> RecoveryRequired`, `Cancelled -> Cancelled`) and
    /// history entry. `Cancelled` is only ever passed here for a *confirmed*
    /// process cancellation (the streaming runner observed the child actually
    /// exit); an unconfirmed cancellation attempt is reported as `Uncertain`
    /// by the runner and already maps to `RecoveryRequired` upstream, never
    /// reaching this outcome.
    pub fn record_planning_result(
        &mut self,
        request: RecordPlanningResultRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let target = target_state_for_planning_outcome(request.outcome);
        validate_plan_text_presence(request.outcome, request.plan_text.as_deref())?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Planning {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        task.transition_to(target, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition =
            self.next_transition(&task, from_state, actor_kind.clone(), reason_code.clone())?;
        let record = TaskPlanningResultRecord {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            outcome: request.outcome,
            exit_code: request.exit_code,
            turn_count: request.turn_count,
            started_at_ms: request.started_at_ms,
            completed_at_ms: occurred_at_ms,
            plan_text: request.plan_text,
        };
        let terminal = target.is_terminal();
        match self.repository.save_planning_result(
            request.expected_version,
            &task,
            &transition,
            &record,
            terminal,
        ) {
            Ok(()) => Ok(TaskView::from(&task)),
            Err(error) => self.recover_after_planning_persistence_failure(
                request.task_id,
                error,
                actor_kind,
                reason_code,
            ),
        }
    }

    /// Falls back to `Planning -> RecoveryRequired` (no `task_planning_results`
    /// row) when the primary atomic result+transition write in
    /// `record_planning_result` fails, mirroring
    /// `GitIsolationService::recover_after_completion_write_failure`'s
    /// "never leave the task silently stuck" pattern.
    fn recover_after_planning_persistence_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskView, ApplicationError> {
        let Ok(Some(mut persisted)) = self.repository.get_task(task_id) else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted.state() != TaskState::Planning {
            return Err(ApplicationError::from_categorized(&original));
        }
        let expected_version = persisted.version();
        let from_state = persisted.state();
        let Ok(now) = self.now_ms() else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted
            .transition_to(TaskState::RecoveryRequired, now)
            .is_err()
        {
            return Err(ApplicationError::from_categorized(&original));
        }
        let Ok(transition) = self.next_transition(&persisted, from_state, actor_kind, reason_code)
        else {
            return Err(ApplicationError::from_categorized(&original));
        };
        match self
            .repository
            .save_transition(expected_version, &persisted, &transition)
        {
            Ok(()) => Ok(TaskView::from(&persisted)),
            Err(_) => Err(ApplicationError::from_categorized(&original)),
        }
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

    /// Startup reconciliation for `Planning`: `PlanningRunRegistry` is
    /// memory-only, so a restart leaves no resumable in-memory execution
    /// handle for whatever task was left `Planning` when the app last ran,
    /// and it must not be treated as still running or auto-resumed. Moves it
    /// to `RecoveryRequired` via the same atomic transition-plus-history path
    /// as [`Self::mark_recovery_required`], keeping the `ActiveTaskLease`. A
    /// no-op (`Ok(None)`) when there is no active task or it is not
    /// `Planning`.
    pub fn reconcile_startup_planning(&mut self) -> Result<Option<TaskView>, ApplicationError> {
        let Some(lease) = self
            .repository
            .active_lease()
            .map_err(|error| ApplicationError::from_categorized(&error))?
        else {
            return Ok(None);
        };
        let task = self
            .repository
            .get_task(lease.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        if task.state() != TaskState::Planning {
            return Ok(None);
        }
        self.mark_recovery_required(TaskActionRequest::new(
            task.id(),
            task.version(),
            "application".to_owned(),
            "planning.startup.recovery-required".to_owned(),
        ))
        .map(Some)
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

const fn target_state_for_planning_outcome(outcome: PlanningResultOutcome) -> TaskState {
    match outcome {
        PlanningResultOutcome::Completed => TaskState::AwaitingDesignApproval,
        PlanningResultOutcome::Failed => TaskState::Failed,
        PlanningResultOutcome::RecoveryRequired => TaskState::RecoveryRequired,
        PlanningResultOutcome::Cancelled => TaskState::Cancelled,
    }
}

fn validate_plan_text_presence(
    outcome: PlanningResultOutcome,
    plan_text: Option<&str>,
) -> Result<(), ApplicationError> {
    let valid = match outcome {
        PlanningResultOutcome::Completed => plan_text.is_some_and(|text| !text.is_empty()),
        _ => plan_text.is_none(),
    };
    if valid {
        Ok(())
    } else {
        Err(category_error(FailureCategory::InvalidInput))
    }
}
