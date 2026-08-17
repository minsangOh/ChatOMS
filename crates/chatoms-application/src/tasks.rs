use std::str::FromStr;

use chatoms_domain::{
    ActorKind, ContextDataScope, DomainError, HighRiskCategory, ProjectId, ReasonCode, Task,
    TaskBranchIdentity, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot, ValidationCommandKind, ValidationExecutionScope, WorkKind,
};
use chatoms_ports::{
    TimeProvider,
    diff::DiffContentHash,
    error::FailureCategory,
    merge_execution::MergeExecutionOutcome,
    provider::ProviderKind,
    repository::{
        ActiveLease, ContextPackagePreparation, DiffApprovalRecord, FoundationRepository,
        GitIsolationStatus, HighRiskApprovalRecord, ImplementationResultOutcome,
        PlanningResultOutcome, PostMergeValidationResultAttempt, PostMergeValidationResultOutcome,
        PostMergeValidationResultRecord, ProviderConsent, ReviewResultOutcome, TaskBriefRecord,
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

pub struct StartMergeRequest {
    task_id: TaskId,
    expected_version: u64,
    actor_kind: String,
    reason_code: String,
}

impl StartMergeRequest {
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

impl StartReviewRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// Starts a Context Package v1 Claude Planning activation from
/// `WorktreeReady`: unlike [`StartPlanningRequest`], this never creates a
/// new provider-transmission consent — the exact `(task_id, Claude,
/// Planning, expected_version, ContextPackageV1)` consent and its FK-bound
/// manifest must already exist (see [`PreparePlanningContextPackageRequest`])
/// — so this request carries no `actor_kind`/`reason_code` for a consent
/// grant, only for the resulting transition history entry.
pub struct StartContextPackagePlanningRequest {
    task_id: TaskId,
    expected_version: u64,
    actor_kind: String,
    reason_code: String,
}

impl StartContextPackagePlanningRequest {
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

/// Whether an exact `(task_id, Claude, Planning, expected_version,
/// ContextPackageV1)` consent and its FK-bound manifest have already been
/// prepared (see [`TaskService::prepare_planning_context_package`]), without
/// creating, reusing, or mutating anything. `ready` is `true` only when both
/// exist; `false` when neither does — the ordinary "not prepared yet" case.
/// A partial pair (exactly one present) is not representable here: see
/// [`TaskService::get_context_package_planning_readiness`], which returns an
/// error instead of a misleading `false` in that case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextPackagePlanningReadiness {
    pub ready: bool,
}

/// Starts a Context Package v1 Claude Implementation activation from
/// `AwaitingDesignApproval`: unlike [`StartImplementationRequest`], this
/// never creates a new provider-transmission consent — the exact `(task_id,
/// Claude, Implementation, expected_version, ContextPackageV1)` consent and
/// its FK-bound manifest must already exist (see
/// [`PrepareImplementationContextPackageRequest`]) — so this request carries
/// no `actor_kind`/`reason_code` for a consent grant, only for the resulting
/// transition history entry.
pub struct StartContextPackageImplementationRequest {
    task_id: TaskId,
    expected_version: u64,
    actor_kind: String,
    reason_code: String,
}

impl StartContextPackageImplementationRequest {
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

/// Whether an exact `(task_id, Claude, Implementation, expected_version,
/// ContextPackageV1)` consent and its FK-bound manifest have already been
/// prepared (see [`TaskService::prepare_implementation_context_package`]),
/// without creating, reusing, or mutating anything. `ready` is `true` only
/// when both exist; `false` when neither does — the ordinary "not prepared
/// yet" case. A partial pair (exactly one present) is not representable
/// here: see [`TaskService::get_context_package_implementation_readiness`],
/// which returns an error instead of a misleading `false` in that case. This
/// deliberately says nothing about whether a completed stored Claude
/// Planning result exists — that is a separate structural precondition
/// [`TaskService::start_context_package_implementation`] checks on its own,
/// not part of what "Context Package v1 prepared" means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextPackageImplementationReadiness {
    pub ready: bool,
}

/// Whether an exact `(task_id, Claude, Review, expected_version,
/// ContextPackageV1)` consent and its FK-bound manifest have already been
/// prepared. See [`ContextPackageImplementationReadiness`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextPackageReviewReadiness {
    pub ready: bool,
}

/// Whether an exact `(task_id, expected_version, risk_category)`
/// [`HighRiskCategory`] approval already exists, without creating, reusing,
/// or mutating it. Unlike [`ContextPackageReviewReadiness`] and its
/// siblings, this is not a consent+manifest pair — a single
/// [`HighRiskApprovalRecord`] either exists or it does not, so there is no
/// partial-pair case to represent as an error. Provider/work-kind/data-scope
/// independent — see [`chatoms_domain::HighRiskCategory`]. Never touches
/// task state, version, transition history, or the `ActiveTaskLease`. This
/// is a dormant use case: no Tauri command, UI, or execution starter calls
/// it yet, and no Policy Engine decides which category applies to which
/// operation — that classification is a future Unit's responsibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HighRiskApprovalStatus {
    pub approved: bool,
}

/// Read-only view of an immutable [`HighRiskApprovalRecord`]. Content-free
/// like the record it mirrors — no free-text description of what was
/// approved, only the closed [`chatoms_domain::HighRiskCategory`]
/// vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HighRiskApprovalView {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub risk_category: HighRiskCategory,
    pub approved_at_ms: i64,
}

impl From<HighRiskApprovalRecord> for HighRiskApprovalView {
    fn from(value: HighRiskApprovalRecord) -> Self {
        Self {
            task_id: value.task_id,
            approved_task_version: value.approved_task_version,
            risk_category: value.risk_category,
            approved_at_ms: value.approved_at_ms,
        }
    }
}

/// Requests an atomic create-or-reuse [`HighRiskCategory`] approval for the
/// exact `(task_id, expected_version, risk_category)` identity. Carries the
/// approval timestamp explicitly (unlike [`StartReviewRequest`], which
/// derives its timestamp from `TimeProvider`) because this use case has no
/// other side effect from which to infer "now" and no state transition to
/// timestamp instead. This request never carries a free-text description of
/// what is being approved — only the closed `risk_category` vocabulary — and
/// approving it never classifies, infers, or expands which category an
/// operation needs; that remains a future Policy Engine's responsibility.
pub struct ApproveHighRiskOperationRequest {
    task_id: TaskId,
    expected_version: u64,
    risk_category: HighRiskCategory,
    approved_at_ms: i64,
}

impl ApproveHighRiskOperationRequest {
    #[must_use]
    pub const fn new(
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
        approved_at_ms: i64,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            risk_category,
            approved_at_ms,
        }
    }
}

/// Read-only view of an immutable [`DiffApprovalRecord`]. Content-free like
/// the record it mirrors — never the raw diff text, only the content-free
/// [`DiffContentHash`] it was approved against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiffApprovalView {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub diff_content_hash: DiffContentHash,
    pub approved_at_ms: i64,
}

impl From<DiffApprovalRecord> for DiffApprovalView {
    fn from(value: DiffApprovalRecord) -> Self {
        Self {
            task_id: value.task_id,
            approved_task_version: value.approved_task_version,
            diff_content_hash: value.diff_content_hash,
            approved_at_ms: value.approved_at_ms,
        }
    }
}

/// Requests an atomic create-or-reuse [`DiffContentHash`]-bound approval for
/// the exact `(task_id, expected_version, diff_content_hash)` identity.
/// Callers (see `chatoms_application::user_diff_approval`) are responsible
/// for recomputing `diff_content_hash` from the task's current worktree
/// diff and verifying it matches whatever hash the user was shown before
/// constructing this request — this request itself carries no diff text
/// and this use case does not read or re-verify any diff.
pub struct RecordDiffApprovalRequest {
    task_id: TaskId,
    expected_version: u64,
    diff_content_hash: DiffContentHash,
    approved_at_ms: i64,
}

impl RecordDiffApprovalRequest {
    #[must_use]
    pub const fn new(
        task_id: TaskId,
        expected_version: u64,
        diff_content_hash: DiffContentHash,
        approved_at_ms: i64,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            diff_content_hash,
            approved_at_ms,
        }
    }
}

/// Prepares (creates or reuses) the exact `ContextPackageV1` consent and its
/// FK-bound manifest for a future Claude Planning attempt, without driving
/// any state transition — `task.state()` must already be `WorktreeReady`
/// and is left there unchanged. This is a distinct, currently-uncalled use
/// case alongside [`StartPlanningRequest`]/[`TaskService::start_planning`],
/// which continues to record and reuse only
/// [`chatoms_domain::ContextDataScope::LegacyPhase4`] and is not modified by
/// this type or [`TaskService::prepare_planning_context_package`].
pub struct PreparePlanningContextPackageRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl PreparePlanningContextPackageRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// See [`PreparePlanningContextPackageRequest`]; requires `task.state() ==
/// AwaitingDesignApproval` instead. `TaskService::start_implementation`
/// remains unmodified and continues to use only `LegacyPhase4`.
pub struct PrepareImplementationContextPackageRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl PrepareImplementationContextPackageRequest {
    #[must_use]
    pub const fn new(task_id: TaskId, expected_version: u64) -> Self {
        Self {
            task_id,
            expected_version,
        }
    }
}

/// See [`PreparePlanningContextPackageRequest`]; requires `task.state() ==
/// Reviewing` instead, matching [`StartReviewRequest`]'s existing
/// no-transition shape. `TaskService::start_review` remains unmodified and
/// continues to use only `LegacyPhase4`.
pub struct PrepareReviewContextPackageRequest {
    task_id: TaskId,
    expected_version: u64,
}

impl PrepareReviewContextPackageRequest {
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

pub struct RecordMergeResultRequest {
    task_id: TaskId,
    expected_version: u64,
    outcome: MergeExecutionOutcome,
    actor_kind: String,
    reason_code: String,
}

impl RecordMergeResultRequest {
    #[must_use]
    pub fn new(
        task_id: TaskId,
        expected_version: u64,
        outcome: MergeExecutionOutcome,
        actor_kind: String,
        reason_code: String,
    ) -> Self {
        Self {
            task_id,
            expected_version,
            outcome,
            actor_kind,
            reason_code,
        }
    }
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

pub struct AppendPostMergeValidationResultRequest {
    task_id: TaskId,
    approval_task_version: u64,
    post_merge_task_version: u64,
    kind: ValidationCommandKind,
    exit_code: Option<i32>,
    safe_summary: String,
    started_at_ms: i64,
    completed_at_ms: i64,
}

impl AppendPostMergeValidationResultRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        approval_task_version: u64,
        post_merge_task_version: u64,
        kind: ValidationCommandKind,
        exit_code: Option<i32>,
        safe_summary: String,
        started_at_ms: i64,
        completed_at_ms: i64,
    ) -> Self {
        Self {
            task_id,
            approval_task_version,
            post_merge_task_version,
            kind,
            exit_code,
            safe_summary,
            started_at_ms,
            completed_at_ms,
        }
    }
}

pub struct FinalizePostMergeValidationBatchRequest {
    task_id: TaskId,
    approval_task_version: u64,
    expected_version: u64,
    kind: ValidationCommandKind,
    outcome: PostMergeValidationResultOutcome,
    exit_code: Option<i32>,
    safe_summary: String,
    started_at_ms: i64,
    actor_kind: String,
    reason_code: String,
}

impl FinalizePostMergeValidationBatchRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: TaskId,
        approval_task_version: u64,
        expected_version: u64,
        kind: ValidationCommandKind,
        outcome: PostMergeValidationResultOutcome,
        exit_code: Option<i32>,
        safe_summary: String,
        started_at_ms: i64,
        actor_kind: String,
        reason_code: String,
    ) -> Self {
        Self {
            task_id,
            approval_task_version,
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
    /// history entry in a single repository transaction. The consent is
    /// always recorded and looked up under [`ContextDataScope::LegacyPhase4`]
    /// — the fixed data shape every current Phase 4 Planning call transmits;
    /// no path in this Unit ever selects [`ContextDataScope::ContextPackageV1`].
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
                ContextDataScope::LegacyPhase4,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let occurred_at_ms = self.now_ms()?;
        let new_consent = existing_consent.is_none().then_some(ProviderConsent {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: request.expected_version,
            data_scope: ContextDataScope::LegacyPhase4,
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

    /// Read-only: reports whether an exact `(task_id, Claude, Planning,
    /// expected_version, ContextPackageV1)` consent and its FK-bound
    /// manifest already exist, without creating, reusing, or mutating
    /// either. Never touches task state, version, transition history, or
    /// the `ActiveTaskLease`. A partial pair (exactly one present — an
    /// already-corrupted invariant [`Self::prepare_planning_context_package`]
    /// itself guards against) is reported as an error, never as a
    /// misleading `ready: false`; a genuine repository failure is likewise
    /// propagated as an error, never silently converted to `false`.
    pub fn get_context_package_planning_readiness(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<ContextPackagePlanningReadiness, ApplicationError> {
        let consent = self
            .repository
            .get_provider_consent(
                task_id,
                ProviderKind::Claude,
                WorkKind::Planning,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let manifest = self
            .repository
            .get_context_package_manifest(
                task_id,
                ProviderKind::Claude,
                WorkKind::Planning,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        match (consent, manifest) {
            (Some(_), Some(_)) => Ok(ContextPackagePlanningReadiness { ready: true }),
            (None, None) => Ok(ContextPackagePlanningReadiness { ready: false }),
            (Some(_), None) | (None, Some(_)) => {
                Err(category_error(FailureCategory::InvariantViolation))
            }
        }
    }

    /// Starts a Context Package v1 Claude Planning activation: verifies the
    /// task is `WorktreeReady` with a verified-ready isolation record (same
    /// preconditions as [`Self::start_planning`]), then verifies — read-only,
    /// via [`Self::get_context_package_planning_readiness`] — that the exact
    /// `(task_id, Claude, Planning, expected_version, ContextPackageV1)`
    /// consent and its FK-bound manifest already exist. If preparation is
    /// missing or the readiness check itself errors, this returns a
    /// fail-closed error and leaves the task exactly `WorktreeReady` —
    /// nothing is written. Only once every precondition passes does this
    /// commit the `WorktreeReady -> Planning` transition and its history
    /// entry via `FoundationRepository::save_context_package_planning_transition`,
    /// which never creates or reuses a consent of its own (unlike
    /// [`Self::start_planning`]) and re-verifies the same consent/manifest
    /// pair again inside its own transaction as defense-in-depth.
    pub fn start_context_package_planning(
        &mut self,
        request: StartContextPackagePlanningRequest,
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
        let readiness =
            self.get_context_package_planning_readiness(request.task_id, request.expected_version)?;
        if !readiness.ready {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        task.transition_to(TaskState::Planning, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        self.repository
            .save_context_package_planning_transition(request.expected_version, &task, &transition)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    /// Starts Claude Implementation: verifies the task is
    /// `AwaitingDesignApproval`, reuses a valid same-version Implementation
    /// consent if one already exists (otherwise records a new one), and
    /// commits the consent (when new), the `Implementing` state update, and
    /// the transition history entry in a single repository transaction. The
    /// consent is always recorded and looked up under
    /// [`ContextDataScope::LegacyPhase4`] — see [`Self::start_planning`].
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
                ContextDataScope::LegacyPhase4,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let occurred_at_ms = self.now_ms()?;
        let new_consent = existing_consent.is_none().then_some(ProviderConsent {
            task_id: request.task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Implementation,
            approved_task_version: request.expected_version,
            data_scope: ContextDataScope::LegacyPhase4,
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

    /// Read-only: reports whether an exact `(task_id, Claude, Implementation,
    /// expected_version, ContextPackageV1)` consent and its FK-bound
    /// manifest already exist, without creating, reusing, or mutating
    /// either. Never touches task state, version, transition history, or
    /// the `ActiveTaskLease`. A partial pair (exactly one present — an
    /// already-corrupted invariant
    /// [`Self::prepare_implementation_context_package`] itself guards
    /// against) is reported as an error, never as a misleading `ready:
    /// false`; a genuine repository failure is likewise propagated as an
    /// error, never silently converted to `false`.
    pub fn get_context_package_implementation_readiness(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<ContextPackageImplementationReadiness, ApplicationError> {
        let consent = self
            .repository
            .get_provider_consent(
                task_id,
                ProviderKind::Claude,
                WorkKind::Implementation,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let manifest = self
            .repository
            .get_context_package_manifest(
                task_id,
                ProviderKind::Claude,
                WorkKind::Implementation,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        match (consent, manifest) {
            (Some(_), Some(_)) => Ok(ContextPackageImplementationReadiness { ready: true }),
            (None, None) => Ok(ContextPackageImplementationReadiness { ready: false }),
            (Some(_), None) | (None, Some(_)) => {
                Err(category_error(FailureCategory::InvariantViolation))
            }
        }
    }

    /// Starts a Context Package v1 Claude Implementation activation:
    /// verifies, read-only and in this fixed order, that the task is
    /// `AwaitingDesignApproval` at `expected_version`; that its isolation is
    /// a verified-ready `WorktreeReady` record; that a `Completed` Claude
    /// Planning result with non-empty `plan_text` is already stored (the
    /// same evidence
    /// [`crate::implementation_execution::ImplementationExecutionStarter::begin`]'s
    /// `load_execution_evidence` requires for the legacy path); that a
    /// `TaskBrief` exists; and finally — via
    /// [`Self::get_context_package_implementation_readiness`] — that the
    /// exact `(task_id, Claude, Implementation, expected_version,
    /// ContextPackageV1)` consent and its FK-bound manifest already exist.
    /// The first four are structural invariants a real
    /// `AwaitingDesignApproval` task must already satisfy, so their absence
    /// is reported the same way `load_execution_evidence` reports it
    /// (`NotFound`/`InvariantViolation`); only the last (context package not
    /// yet prepared) is the ordinary "not ready yet" case, reported as
    /// `InvalidState`. If any check fails, this returns a fail-closed error
    /// and leaves the task exactly `AwaitingDesignApproval` — nothing is
    /// written. Only once every precondition passes does this commit the
    /// `AwaitingDesignApproval -> Implementing` transition and its history
    /// entry via
    /// `FoundationRepository::save_context_package_implementation_transition`,
    /// which never creates or reuses a consent of its own (unlike
    /// [`Self::start_implementation`]) and re-verifies the same
    /// consent/manifest/planning-result evidence again inside its own
    /// transaction as defense-in-depth.
    pub fn start_context_package_implementation(
        &mut self,
        request: StartContextPackageImplementationRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::AwaitingDesignApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let isolation = self
            .repository
            .get_task_isolation(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        let planning_result = self
            .repository
            .get_task_planning_result(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::NotFound))?;
        if planning_result.outcome != PlanningResultOutcome::Completed {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        if !planning_result
            .plan_text
            .as_deref()
            .is_some_and(|text| !text.is_empty())
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }
        self.repository
            .get_task_brief(request.task_id)
            .map_err(|error| ApplicationError::from_categorized(&error))?
            .ok_or_else(|| category_error(FailureCategory::InvariantViolation))?;
        let readiness = self.get_context_package_implementation_readiness(
            request.task_id,
            request.expected_version,
        )?;
        if !readiness.ready {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let occurred_at_ms = self.now_ms()?;
        task.transition_to(TaskState::Implementing, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        self.repository
            .save_context_package_implementation_transition(
                request.expected_version,
                &task,
                &transition,
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
    /// `ActiveTaskLease`. The consent is always recorded and looked up under
    /// [`ContextDataScope::LegacyPhase4`] — see [`Self::start_planning`].
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
            .save_review_consent(
                request.expected_version,
                request.task_id,
                ContextDataScope::LegacyPhase4,
                occurred_at_ms,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    pub fn start_merge(
        &mut self,
        request: StartMergeRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::AwaitingUserDiffApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        task.transition_to(TaskState::Merging, self.now_ms()?)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition = self.next_transition(&task, from_state, actor_kind, reason_code)?;
        self.repository
            .save_transition(request.expected_version, &task, &transition)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(TaskView::from(&task))
    }

    /// Read-only: reports whether an exact `(task_id, Claude, Review,
    /// expected_version, ContextPackageV1)` consent and its FK-bound
    /// manifest already exist, without creating, reusing, or mutating
    /// either. Never touches task state, version, transition history, or the
    /// `ActiveTaskLease`. A partial pair (exactly one present — an
    /// already-corrupted invariant [`Self::prepare_review_context_package`]
    /// itself guards against) is reported as an error, never as a
    /// misleading `ready: false`; a genuine repository failure is likewise
    /// propagated as an error, never silently converted to `false`. Mirrors
    /// [`Self::get_context_package_implementation_readiness`] exactly.
    pub fn get_context_package_review_readiness(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
    ) -> Result<ContextPackageReviewReadiness, ApplicationError> {
        let consent = self
            .repository
            .get_provider_consent(
                task_id,
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let manifest = self
            .repository
            .get_context_package_manifest(
                task_id,
                ProviderKind::Claude,
                WorkKind::Review,
                expected_version,
                ContextDataScope::ContextPackageV1,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        match (consent, manifest) {
            (Some(_), Some(_)) => Ok(ContextPackageReviewReadiness { ready: true }),
            (None, None) => Ok(ContextPackageReviewReadiness { ready: false }),
            (Some(_), None) | (None, Some(_)) => {
                Err(category_error(FailureCategory::InvariantViolation))
            }
        }
    }

    /// Read-only: reports whether an exact `(task_id, expected_version,
    /// risk_category)` [`HighRiskCategory`] approval already exists, without
    /// creating, reusing, or mutating it. Verifies the task exists and is at
    /// `expected_version` first; a version mismatch or a genuine repository
    /// failure (including a corrupted persisted category — see
    /// [`chatoms_ports::repository::FoundationRepository::get_high_risk_approval`])
    /// is propagated as an error, never silently converted to `approved:
    /// false`. Never touches task state, version, transition history, or the
    /// `ActiveTaskLease`. Dormant: no Tauri command, UI, or execution
    /// starter calls this yet.
    pub fn get_high_risk_approval_status(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<HighRiskApprovalStatus, ApplicationError> {
        self.load_expected_task(task_id, expected_version)?;
        let approval = self
            .repository
            .get_high_risk_approval(task_id, expected_version, risk_category)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(HighRiskApprovalStatus {
            approved: approval.is_some(),
        })
    }

    /// Atomically creates-or-reuses the exact `(task_id, expected_version,
    /// risk_category)` [`HighRiskCategory`] approval: verifies the task
    /// exists and is at `expected_version`, then delegates the entire
    /// read-existing-or-insert-new decision to
    /// [`chatoms_ports::repository::FoundationRepository::ensure_high_risk_approval`],
    /// which re-verifies the version and performs the create-or-reuse
    /// atomically inside its own transaction. The returned view never
    /// distinguishes "just created" from "already existed" — both are the
    /// same successful outcome — and this method never infers, classifies,
    /// or expands `risk_category` from anything other than the caller's
    /// explicit request. Never touches task state, transition history, or
    /// the `ActiveTaskLease`, and calls no provider consent, manifest, or
    /// validation-approval use case. Dormant: no Tauri command, UI, or
    /// execution starter calls this yet, and no Policy Engine decides when
    /// it should be called.
    pub fn approve_high_risk_operation(
        &mut self,
        request: ApproveHighRiskOperationRequest,
    ) -> Result<HighRiskApprovalView, ApplicationError> {
        self.load_expected_task(request.task_id, request.expected_version)?;
        let approval = self
            .repository
            .ensure_high_risk_approval(
                request.task_id,
                request.expected_version,
                request.risk_category,
                request.approved_at_ms,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(HighRiskApprovalView::from(approval))
    }

    /// Atomically creates-or-reuses the exact `(task_id, expected_version,
    /// diff_content_hash)` approval: verifies the task exists and is at
    /// `expected_version`, then delegates the entire
    /// read-existing-or-insert-new decision to
    /// [`chatoms_ports::repository::FoundationRepository::ensure_diff_approval`],
    /// which re-verifies the version and performs the create-or-reuse
    /// atomically inside its own transaction. This method does not read or
    /// re-verify the worktree diff itself — callers (see
    /// `chatoms_application::user_diff_approval::UserDiffApprovalService`)
    /// must have already recomputed the current diff's hash and confirmed
    /// it matches before calling this. Never touches task state, transition
    /// history, or the `ActiveTaskLease`, and calls no provider consent,
    /// manifest, high-risk-approval, or Merging use case.
    pub fn record_diff_approval(
        &mut self,
        request: RecordDiffApprovalRequest,
    ) -> Result<DiffApprovalView, ApplicationError> {
        self.load_expected_task(request.task_id, request.expected_version)?;
        let approval = self
            .repository
            .ensure_diff_approval(
                request.task_id,
                request.expected_version,
                request.diff_content_hash,
                request.approved_at_ms,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        Ok(DiffApprovalView::from(approval))
    }

    /// Prepares (creates or reuses) the exact `(task_id, Claude, Planning,
    /// expected_version, ContextPackageV1)` consent together with its
    /// FK-bound manifest, atomically, via
    /// `FoundationRepository::prepare_planning_context_package`. Never
    /// drives a state transition, never touches transition history or the
    /// `ActiveTaskLease`, and never creates, reuses, or otherwise touches a
    /// [`ContextDataScope::LegacyPhase4`] consent — this is a thin wrapper
    /// around a repository method that is itself the sole owner of the
    /// create-or-reuse-or-fail-closed decision (see that method's contract
    /// for the partial-state fail-closed rule). This use case is not called
    /// by any Tauri command, adapter, or execution starter yet.
    pub fn prepare_planning_context_package(
        &mut self,
        request: PreparePlanningContextPackageRequest,
    ) -> Result<ContextPackagePreparation, ApplicationError> {
        let task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::WorktreeReady {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let prepared_at_ms = self.now_ms()?;
        self.repository
            .prepare_planning_context_package(
                request.expected_version,
                request.task_id,
                prepared_at_ms,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    /// See [`Self::prepare_planning_context_package`]; requires
    /// `task.state() == AwaitingDesignApproval` and delegates to
    /// `FoundationRepository::prepare_implementation_context_package`.
    /// `TaskService::start_implementation` remains unmodified.
    pub fn prepare_implementation_context_package(
        &mut self,
        request: PrepareImplementationContextPackageRequest,
    ) -> Result<ContextPackagePreparation, ApplicationError> {
        let task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::AwaitingDesignApproval {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let prepared_at_ms = self.now_ms()?;
        self.repository
            .prepare_implementation_context_package(
                request.expected_version,
                request.task_id,
                prepared_at_ms,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    /// See [`Self::prepare_planning_context_package`]; requires
    /// `task.state() == Reviewing` and delegates to
    /// `FoundationRepository::prepare_review_context_package`.
    /// `TaskService::start_review` remains unmodified.
    pub fn prepare_review_context_package(
        &mut self,
        request: PrepareReviewContextPackageRequest,
    ) -> Result<ContextPackagePreparation, ApplicationError> {
        let task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Reviewing {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let prepared_at_ms = self.now_ms()?;
        self.repository
            .prepare_review_context_package(
                request.expected_version,
                request.task_id,
                prepared_at_ms,
            )
            .map_err(|error| ApplicationError::from_categorized(&error))
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

    pub fn record_merge_result(
        &mut self,
        request: RecordMergeResultRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::Merging {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let target = match request.outcome {
            MergeExecutionOutcome::Merged => TaskState::PostMergeTesting,
            MergeExecutionOutcome::ConfirmedMergeConflict => TaskState::MergeConflict,
            MergeExecutionOutcome::PreWriteRejected(_)
            | MergeExecutionOutcome::StageWriteUncertain
            | MergeExecutionOutcome::CommitNotCreated
            | MergeExecutionOutcome::CommitSucceededMergeFailed
            | MergeExecutionOutcome::MergeConflictResidue
            | MergeExecutionOutcome::PostWriteUncertain => TaskState::RecoveryRequired,
        };
        task.transition_to(target, self.now_ms()?)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition =
            self.next_transition(&task, from_state, actor_kind.clone(), reason_code.clone())?;
        match self
            .repository
            .save_transition(request.expected_version, &task, &transition)
        {
            Ok(()) => Ok(TaskView::from(&task)),
            Err(error) => self.recover_after_merge_persistence_failure(
                request.task_id,
                error,
                actor_kind,
                reason_code,
            ),
        }
    }

    fn recover_after_merge_persistence_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskView, ApplicationError> {
        let Ok(Some(mut persisted)) = self.repository.get_task(task_id) else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted.state() != TaskState::Merging {
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
            execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
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

    pub fn append_post_merge_validation_result(
        &mut self,
        request: AppendPostMergeValidationResultRequest,
    ) -> Result<PostMergeValidationResultRecord, ApplicationError> {
        let task = self.load_expected_task(request.task_id, request.post_merge_task_version)?;
        if task.state() != TaskState::PostMergeTesting {
            return Err(category_error(FailureCategory::InvalidState));
        }
        self.repository
            .append_post_merge_validation_result(&PostMergeValidationResultAttempt {
                task_id: request.task_id,
                approval_task_version: request.approval_task_version,
                post_merge_task_version: request.post_merge_task_version,
                execution_scope: ValidationExecutionScope::ProjectRoot,
                kind: request.kind,
                outcome: PostMergeValidationResultOutcome::Success,
                exit_code: request.exit_code,
                safe_summary: request.safe_summary,
                started_at_ms: request.started_at_ms,
                completed_at_ms: request.completed_at_ms,
            })
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn finalize_post_merge_validation_batch(
        &mut self,
        request: FinalizePostMergeValidationBatchRequest,
    ) -> Result<TaskView, ApplicationError> {
        let actor_kind = parse_actor(&request.actor_kind)?;
        let reason_code = parse_reason(&request.reason_code)?;
        let mut task = self.load_expected_task(request.task_id, request.expected_version)?;
        if task.state() != TaskState::PostMergeTesting {
            return Err(category_error(FailureCategory::InvalidState));
        }
        let from_state = task.state();
        let target = if request.outcome == PostMergeValidationResultOutcome::Success {
            TaskState::Completed
        } else {
            TaskState::RecoveryRequired
        };
        let occurred_at_ms = self.now_ms()?;
        task.transition_to(target, occurred_at_ms)
            .map_err(|error| ApplicationError::from_domain(&error))?;
        let transition =
            self.next_transition(&task, from_state, actor_kind.clone(), reason_code.clone())?;
        let attempt = PostMergeValidationResultAttempt {
            task_id: request.task_id,
            approval_task_version: request.approval_task_version,
            post_merge_task_version: request.expected_version,
            execution_scope: ValidationExecutionScope::ProjectRoot,
            kind: request.kind,
            outcome: request.outcome,
            exit_code: request.exit_code,
            safe_summary: request.safe_summary,
            started_at_ms: request.started_at_ms,
            completed_at_ms: occurred_at_ms,
        };
        match self.repository.finalize_post_merge_validation_batch(
            request.expected_version,
            &task,
            &transition,
            &attempt,
        ) {
            Ok(()) => Ok(TaskView::from(&task)),
            Err(error) => self.recover_after_post_merge_validation_persistence_failure(
                request.task_id,
                error,
                actor_kind,
                reason_code,
            ),
        }
    }

    fn recover_after_post_merge_validation_persistence_failure(
        &mut self,
        task_id: TaskId,
        original: chatoms_ports::repository::RepositoryError,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
    ) -> Result<TaskView, ApplicationError> {
        let Ok(Some(mut persisted)) = self.repository.get_task(task_id) else {
            return Err(ApplicationError::from_categorized(&original));
        };
        if persisted.state() != TaskState::PostMergeTesting {
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
