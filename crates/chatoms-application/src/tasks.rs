use std::str::FromStr;

use chatoms_domain::{
    ActorKind, DomainError, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId, TaskState,
    TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot, ValidationCommandKind,
    WorkKind,
};
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    provider::ProviderKind,
    repository::{
        ActiveLease, FoundationRepository, GitIsolationStatus, ImplementationResultOutcome,
        PlanningResultOutcome, ProviderConsent, ReviewResultOutcome, TaskBriefRecord,
        TaskImplementationResultRecord, TaskPlanningResultRecord, TaskReviewResultRecord,
        ValidationCommandResultAttempt, ValidationCommandResultOutcome,
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

/// Starts Claude Implementation from `AwaitingDesignApproval`.
/// `actor_kind`/`reason_code` describe who approved the provider-transmission
/// consent and why.
pub struct StartImplementationRequest {
    task_id: TaskId,
    expected_version: u64,
    actor_kind: String,
    reason_code: String,
}

impl StartImplementationRequest {
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

/// Starts Claude Review consent from `Reviewing`. Unlike
/// [`StartPlanningRequest`]/[`StartImplementationRequest`], this never drives
/// a state transition — `Testing -> Reviewing` already happened
/// automatically (see `TaskService::finalize_validation_command_batch`) — so
/// there is no transition history entry to attribute to an actor/reason, and
/// this request carries neither.
pub struct StartReviewRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl StartReviewRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
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

/// Records a Claude Implementation attempt's already-safe outcome (reduced
/// by the infrastructure adapter — this request never carries raw provider
/// output, a diff, or any other content field) and drives the resulting
/// `Implementing -> Testing/Paused/RecoveryRequired` transition atomically
/// with it. `started_at_ms` is caller-supplied because only the caller (the
/// future orchestrator that invoked the adapter) observed when the attempt
/// began; `completed_at_ms` is sourced from this service's own
/// `TimeProvider`.
pub struct RecordImplementationResultRequest {
    task_id: TaskId,
    expected_version: u64,
    outcome: ImplementationResultOutcome,
    exit_code: Option<i32>,
    turn_count: Option<u32>,
    started_at_ms: i64,
    actor_kind: String,
    reason_code: String,
}

impl RecordImplementationResultRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        outcome: ImplementationResultOutcome,
        exit_code: Option<i32>,
        turn_count: Option<u32>,
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
            started_at_ms,
            actor_kind,
            reason_code,
        }
    }
}

/// Records a Claude Review attempt's already-safe outcome (parsed, masked,
/// and size-capped by the infrastructure adapter — this request never
/// carries raw provider output, a raw Git diff, or any other unsafe content
/// field) and drives the resulting `Reviewing -> AwaitingUserDiffApproval`
/// (`Completed`), `Reviewing -> Failed` (`Failed`), confirmed `Reviewing ->
/// Paused` (`Cancelled`), or `Reviewing -> RecoveryRequired`
/// (`RecoveryRequired`) transition atomically with it. `started_at_ms` is
/// caller-supplied because only the caller (the future orchestrator that
/// invoked the adapter) observed when the attempt began; `completed_at_ms`
/// is sourced from this service's own `TimeProvider`.
pub struct RecordReviewResultRequest {
    task_id: TaskId,
    expected_version: u64,
    outcome: ReviewResultOutcome,
    exit_code: Option<i32>,
    turn_count: Option<u32>,
    review_text: Option<String>,
    started_at_ms: i64,
    actor_kind: String,
    reason_code: String,
}

impl RecordReviewResultRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        outcome: ReviewResultOutcome,
        exit_code: Option<i32>,
        turn_count: Option<u32>,
        review_text: Option<String>,
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
            review_text,
            started_at_ms,
            actor_kind,
            reason_code,
        }
    }
}

/// Finalizes one Testing batch attempt: appends the batch's *final*
/// validation command result (already reduced to a safe, fixed-vocabulary
/// outcome — never raw stdout/stderr) and, in the same repository
/// transaction, drives the resulting state transition (`Success ->
/// Reviewing`, confirmed `Cancelled -> Paused` with `resume_target_state =
/// Testing`, every other outcome -> `RecoveryRequired`) and history entry.
/// Whether `outcome` is "the last approved command succeeded" or "the first
/// non-success/cancelled command" is decided by the caller (see
/// `chatoms_application::testing_execution`) — this request only carries
/// the one outcome that ends the batch. `started_at_ms` is caller-supplied
/// (only the caller observed when this specific attempt began);
/// `completed_at_ms` is sourced from this service's own `TimeProvider`.
pub struct FinalizeValidationCommandBatchRequest {
    task_id: TaskId,
    expected_version: u64,
    kind: ValidationCommandKind,
    outcome: ValidationCommandResultOutcome,
    exit_code: Option<i32>,
    safe_summary: String,
    started_at_ms: i64,
    actor_kind: String,
    reason_code: String,
}

impl FinalizeValidationCommandBatchRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        kind: ValidationCommandKind,
        outcome: ValidationCommandResultOutcome,
        exit_code: Option<i32>,
        safe_summary: String,
        started_at_ms: i64,
        actor_kind: String,
        reason_code: String,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            kind,
            outcome,
            exit_code,
            safe_summary,
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

/// Read-only view of an already-safe, immutable Claude Review result (see
/// [`TaskReviewResultRecord`] for the masking/size-bound guarantees this is
/// a direct passthrough of). Never carries raw provider output or a raw
/// Git diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewResultView {
    pub outcome: ReviewResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub review_text: Option<String>,
}

impl From<TaskReviewResultRecord> for ReviewResultView {
    fn from(value: TaskReviewResultRecord) -> Self {
        Self {
            outcome: value.outcome,
            exit_code: value.exit_code,
            turn_count: value.turn_count,
            started_at_ms: value.started_at_ms,
            completed_at_ms: value.completed_at_ms,
            review_text: value.review_text,
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

    /// Reads back the immutable Claude Review result for `task_id`, if any
    /// attempt has been recorded. A pure passthrough of the already-safe
    /// stored record — callers that only want to expose this in
    /// `AwaitingUserDiffApproval` must apply that gating themselves (see
    /// `src-tauri/src/commands/review.rs`).
    pub fn get_review_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<ReviewResultView>, ApplicationError> {
        self.repository
            .get_task_review_result(task_id)
            .map(|result| result.map(ReviewResultView::from))
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

    /// Starts Claude Implementation: verifies the task is
    /// `AwaitingDesignApproval`, reuses a valid same-version Implementation
    /// consent if one already exists (otherwise records a new one), and
    /// commits the consent (when new), the `Implementing` state update, and
    /// the transition history entry in a single repository transaction.
    pub fn start_implementation(
        &mut self,
        request: StartImplementationRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::AwaitingDesignApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let existing_consent = self
            .repository
            .get_provider_consent(
                request.task_id,
                ProviderKind::Claude,
                WorkKind::Implementation,
                request.expected_version,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let occurred_at_ms = self.now_ms()?;
        let new_consent = existing_consent.is_none().then_some(ProviderConsent {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Implementation,
            approved_task_version: request.expected_version,
            consented_at_ms: occurred_at_ms,
        });
        let from_state = task.state();
        task.transition_to(TaskState::Implementing, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        self.repository
            .save_implementation_transition(
                request.expected_version,
                &task,
                &transition,
                new_consent.as_ref(),
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    /// Starts Claude Review: verifies the task is `Reviewing` at the expected
    /// version, then records or reuses a same-version Claude/Review consent.
    /// Unlike [`Self::start_planning`]/[`Self::start_implementation`], there
    /// is no state transition to commit — `Testing -> Reviewing` already
    /// happened automatically (see [`Self::finalize_validation_command_batch`])
    /// — so this delegates the entire read-existing-or-insert-new decision to
    /// `FoundationRepository::save_review_consent`, which re-verifies the
    /// task's version and `Reviewing` state inside its own transaction and
    /// never touches task state, version, transition history, or the
    /// `ActiveTaskLease`.
    pub fn start_review(
        &mut self,
        request: StartReviewRequest,
    ) -> Result<TaskView, ApplicationError> {
        let task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Reviewing {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let occurred_at_ms = self.now_ms()?;
        self.repository
            .save_review_consent(request.expected_version, request.task_id, occurred_at_ms)
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

    /// Records a Claude Implementation attempt's already-safe outcome and, in
    /// the same repository transaction, drives the resulting state
    /// transition (`Completed -> Testing`, confirmed `Cancelled -> Paused`
    /// with `resume_target_state = Implementing`, every other outcome ->
    /// `RecoveryRequired`) and history entry. None of these target states is
    /// terminal, so the `ActiveTaskLease` is always kept. `Cancelled` is only
    /// ever passed here for a *confirmed* process cancellation; an
    /// unconfirmed cancellation attempt is already reduced to
    /// `RecoveryRequired` upstream, never reaching this outcome.
    pub fn record_implementation_result(
        &mut self,
        request: RecordImplementationResultRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Implementing {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        apply_implementation_outcome(&mut task, request.outcome, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition =
            self.next_transition(&task, from_state, actor_kind.clone(), reason_code.clone())?;
        let record = TaskImplementationResultRecord {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Implementation,
            outcome: request.outcome,
            exit_code: request.exit_code,
            turn_count: request.turn_count,
            started_at_ms: request.started_at_ms,
            completed_at_ms: occurred_at_ms,
        };
        match self.repository.save_implementation_result(
            request.expected_version,
            &task,
            &transition,
            &record,
        ) {
            Ok(()) => Ok(TaskView::from(&task)),
            Err(error) => self.recover_after_implementation_persistence_failure(
                request.task_id,
                error,
                actor_kind,
                reason_code,
            ),
        }
    }

    /// Falls back to `Implementing -> RecoveryRequired` (no
    /// `task_implementation_results` row) when the primary atomic
    /// result+transition write in `record_implementation_result` fails,
    /// mirroring `recover_after_planning_persistence_failure`'s "never leave
    /// the task silently stuck" pattern.
    fn recover_after_implementation_persistence_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskView, ApplicationError> {
        let Ok(Some(mut persisted)) = self.repository.get_task(task_id) else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted.state() != TaskState::Implementing {
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

    /// Records a Claude Review attempt's already-safe outcome and, in the
    /// same repository transaction, drives the resulting state transition
    /// (`Completed -> AwaitingUserDiffApproval`, `Failed -> Failed`,
    /// confirmed `Cancelled -> Paused` with `resume_target_state =
    /// Reviewing`, `RecoveryRequired -> RecoveryRequired`) and history entry.
    /// `Failed` is the only terminal outcome among these four (Review is
    /// read-only like Planning, so a confirmed failure carries no partial
    /// external effect and can safely release the `ActiveTaskLease`); the
    /// other three keep it. `Cancelled` is only ever passed here for a
    /// *confirmed* process cancellation; an unconfirmed cancellation attempt
    /// is already reduced to `RecoveryRequired` upstream, never reaching this
    /// outcome. `Completed` always reaches `AwaitingUserDiffApproval`
    /// regardless of findings — routing findings into `ReviewFixing` is a
    /// later Unit's responsibility, not this one's.
    pub fn record_review_result(
        &mut self,
        request: RecordReviewResultRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        validate_review_text_presence(request.outcome, request.review_text.as_deref())?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Reviewing {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        apply_review_outcome(&mut task, request.outcome, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition =
            self.next_transition(&task, from_state, actor_kind.clone(), reason_code.clone())?;
        let record = TaskReviewResultRecord {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            outcome: request.outcome,
            exit_code: request.exit_code,
            turn_count: request.turn_count,
            started_at_ms: request.started_at_ms,
            completed_at_ms: occurred_at_ms,
            review_text: request.review_text,
        };
        let terminal = task.state().is_terminal();
        match self.repository.save_review_result(
            request.expected_version,
            &task,
            &transition,
            &record,
            terminal,
        ) {
            Ok(()) => Ok(TaskView::from(&task)),
            Err(error) => self.recover_after_review_persistence_failure(
                request.task_id,
                error,
                actor_kind,
                reason_code,
            ),
        }
    }

    /// Falls back to `Reviewing -> RecoveryRequired` (no `task_review_results`
    /// row) when the primary atomic result+transition write in
    /// `record_review_result` fails, mirroring
    /// `recover_after_planning_persistence_failure`'s "never leave the task
    /// silently stuck" pattern.
    fn recover_after_review_persistence_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskView, ApplicationError> {
        let Ok(Some(mut persisted)) = self.repository.get_task(task_id) else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted.state() != TaskState::Reviewing {
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

    /// Finalizes one Testing batch attempt: appends the batch's final
    /// validation command result and, in the same repository transaction,
    /// drives the resulting state transition (`Success -> Reviewing`,
    /// confirmed `Cancelled -> Paused` with `resume_target_state =
    /// Testing`, every other outcome -> `RecoveryRequired`) and history
    /// entry. None of these target states is terminal, so the
    /// `ActiveTaskLease` is always kept.
    pub fn finalize_validation_command_batch(
        &mut self,
        request: FinalizeValidationCommandBatchRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Testing {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        apply_testing_batch_outcome(&mut task, request.outcome, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition =
            self.next_transition(&task, from_state, actor_kind.clone(), reason_code.clone())?;
        let attempt = ValidationCommandResultAttempt {
            task_id: request.task_id,
            approved_task_version: request.expected_version,
            kind: request.kind,
            outcome: request.outcome,
            exit_code: request.exit_code,
            safe_summary: request.safe_summary,
            started_at_ms: request.started_at_ms,
            completed_at_ms: occurred_at_ms,
        };
        match self.repository.finalize_validation_command_batch(
            request.expected_version,
            &task,
            &transition,
            &attempt,
        ) {
            Ok(()) => Ok(TaskView::from(&task)),
            Err(error) => self.recover_after_validation_batch_persistence_failure(
                request.task_id,
                error,
                actor_kind,
                reason_code,
            ),
        }
    }

    /// Falls back to `Testing -> RecoveryRequired` (no
    /// `task_validation_command_results` row) when the primary atomic
    /// result+transition write in `finalize_validation_command_batch` fails,
    /// mirroring `recover_after_implementation_persistence_failure`'s "never
    /// leave the task silently stuck" pattern.
    fn recover_after_validation_batch_persistence_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskView, ApplicationError> {
        let Ok(Some(mut persisted)) = self.repository.get_task(task_id) else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted.state() != TaskState::Testing {
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

    /// Startup reconciliation for `Testing`, mirroring
    /// [`Self::reconcile_startup_planning`] exactly: `TestingRunRegistry` (the
    /// Tauri-layer analog of `PlanningRunRegistry` for Cargo-only validation
    /// batches) is memory-only, so a restart leaves no resumable in-memory
    /// execution handle for whatever task was left `Testing` when the app
    /// last ran, and it must not be treated as still running or
    /// auto-resumed. Moves it to `RecoveryRequired` via the same atomic
    /// transition-plus-history path as [`Self::mark_recovery_required`],
    /// keeping the `ActiveTaskLease`. A no-op (`Ok(None)`) when there is no
    /// active task or it is not `Testing`.
    pub fn reconcile_startup_testing(&mut self) -> Result<Option<TaskView>, ApplicationError> {
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
        if task.state() != TaskState::Testing {
            return Ok(None);
        }
        self.mark_recovery_required(TaskActionRequest::new(
            task.id(),
            task.version(),
            "application".to_owned(),
            "testing.startup.recovery-required".to_owned(),
        ))
        .map(Some)
    }

    /// Startup reconciliation for `Reviewing`, mirroring
    /// [`Self::reconcile_startup_planning`]/[`Self::reconcile_startup_testing`]
    /// exactly: `ReviewRunRegistry` (the Tauri-layer analog of
    /// `PlanningRunRegistry`/`TestingRunRegistry` for Claude Review runs) is
    /// memory-only, so a restart leaves no resumable in-memory execution
    /// handle for whatever task was left `Reviewing` when the app last ran,
    /// and it must not be treated as still running or auto-resumed. Moves it
    /// to `RecoveryRequired` via the same atomic transition-plus-history path
    /// as [`Self::mark_recovery_required`], keeping the `ActiveTaskLease`. A
    /// no-op (`Ok(None)`) when there is no active task or it is not
    /// `Reviewing`.
    pub fn reconcile_startup_reviewing(&mut self) -> Result<Option<TaskView>, ApplicationError> {
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
        if task.state() != TaskState::Reviewing {
            return Ok(None);
        }
        self.mark_recovery_required(TaskActionRequest::new(
            task.id(),
            task.version(),
            "application".to_owned(),
            "reviewing.startup.recovery-required".to_owned(),
        ))
        .map(Some)
    }

    /// Startup reconciliation for `Implementing`, mirroring
    /// [`Self::reconcile_startup_planning`]/[`Self::reconcile_startup_testing`]/
    /// [`Self::reconcile_startup_reviewing`] exactly: `ImplementationRunRegistry`
    /// (the Tauri-layer analog of `PlanningRunRegistry`/`TestingRunRegistry`/
    /// `ReviewRunRegistry` for Claude Implementation runs) is memory-only, so
    /// a restart leaves no resumable in-memory execution handle for whatever
    /// task was left `Implementing` when the app last ran, and it must not be
    /// treated as still running or auto-resumed. Moves it to
    /// `RecoveryRequired` via the same atomic transition-plus-history path as
    /// [`Self::mark_recovery_required`], keeping the `ActiveTaskLease`. A
    /// no-op (`Ok(None)`) when there is no active task or it is not
    /// `Implementing`.
    pub fn reconcile_startup_implementation(
        &mut self,
    ) -> Result<Option<TaskView>, ApplicationError> {
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
        if task.state() != TaskState::Implementing {
            return Ok(None);
        }
        self.mark_recovery_required(TaskActionRequest::new(
            task.id(),
            task.version(),
            "application".to_owned(),
            "implementation.startup.recovery-required".to_owned(),
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

/// Applies a Claude Implementation outcome's approved state transition to
/// `task`. `Cancelled` goes through [`Task::pause`] (not
/// [`Task::transition_to`]) because pausing is the mechanism that sets
/// `resume_target_state = Implementing`, matching the approved "confirmed
/// cancellation -> `Paused`, resumable back to `Implementing`" mapping.
fn apply_implementation_outcome(
    task: &mut Task,
    outcome: ImplementationResultOutcome,
    occurred_at_ms: i64,
) -> Result<(), DomainError> {
    match outcome {
        ImplementationResultOutcome::Completed => {
            task.transition_to(TaskState::Testing, occurred_at_ms)
        }
        ImplementationResultOutcome::Cancelled => task.pause(occurred_at_ms),
        ImplementationResultOutcome::RecoveryRequired => {
            task.transition_to(TaskState::RecoveryRequired, occurred_at_ms)
        }
    }
}

/// Applies a Testing batch's final outcome to `task`. `Cancelled` goes
/// through [`Task::pause`] (not [`Task::transition_to`]), matching
/// [`apply_implementation_outcome`]'s reasoning: pausing is the mechanism
/// that sets `resume_target_state = Testing`, the approved "confirmed
/// cancellation -> `Paused`, resumable back to `Testing`" mapping.
/// `Success` here always means "every approved command in the batch
/// succeeded" (the caller — `chatoms_application::testing_execution` —
/// never reaches this for an intermediate success).
fn apply_testing_batch_outcome(
    task: &mut Task,
    outcome: ValidationCommandResultOutcome,
    occurred_at_ms: i64,
) -> Result<(), DomainError> {
    match outcome {
        ValidationCommandResultOutcome::Success => {
            task.transition_to(TaskState::Reviewing, occurred_at_ms)
        }
        ValidationCommandResultOutcome::Cancelled => task.pause(occurred_at_ms),
        ValidationCommandResultOutcome::ExitFailure
        | ValidationCommandResultOutcome::TimedOut
        | ValidationCommandResultOutcome::StdoutBoundExceeded
        | ValidationCommandResultOutcome::Uncertain => {
            task.transition_to(TaskState::RecoveryRequired, occurred_at_ms)
        }
    }
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

/// Applies a Claude Review outcome's approved state transition to `task`.
/// `Cancelled` goes through [`Task::pause`] (not [`Task::transition_to`]),
/// matching [`apply_implementation_outcome`]/[`apply_testing_batch_outcome`]'s
/// reasoning: pausing is the mechanism that sets `resume_target_state =
/// Reviewing`. Unlike those two (write-capable work), Review is read-only
/// like Planning (`--tools Read,Glob,Grep` only), so a confirmed `Failed`
/// maps directly to the terminal `Failed` state instead of being folded into
/// `RecoveryRequired`. `Completed` always reaches
/// `AwaitingUserDiffApproval` — domain has no `Reviewing -> Completed` edge,
/// and routing findings into `ReviewFixing` is a later Unit's
/// responsibility.
fn apply_review_outcome(
    task: &mut Task,
    outcome: ReviewResultOutcome,
    occurred_at_ms: i64,
) -> Result<(), DomainError> {
    match outcome {
        ReviewResultOutcome::Completed => {
            task.transition_to(TaskState::AwaitingUserDiffApproval, occurred_at_ms)
        }
        ReviewResultOutcome::Failed => task.transition_to(TaskState::Failed, occurred_at_ms),
        ReviewResultOutcome::Cancelled => task.pause(occurred_at_ms),
        ReviewResultOutcome::RecoveryRequired => {
            task.transition_to(TaskState::RecoveryRequired, occurred_at_ms)
        }
    }
}

fn validate_review_text_presence(
    outcome: ReviewResultOutcome,
    review_text: Option<&str>,
) -> Result<(), ApplicationError> {
    let valid = match outcome {
        ReviewResultOutcome::Completed => review_text.is_some_and(|text| !text.is_empty()),
        _ => review_text.is_none(),
    };
    if valid {
        Ok(())
    } else {
        Err(category_error(FailureCategory::InvalidInput))
    }
}
