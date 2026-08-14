use std::{error::Error, fmt};

use chatoms_domain::{
    GitOperationId, ProjectId, Task, TaskId, TaskStateTransition, ValidationCommandKind, WorkKind,
};

use crate::git::RepositoryKind;
use crate::provider::ProviderKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryErrorCode {
    ProjectNotFound,
    DuplicateProject,
    TaskNotFound,
    DuplicateTask,
    IsolationNotFound,
    BindingNotFound,
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
pub struct TaskBriefRecord {
    pub task_id: TaskId,
    pub requirements: String,
    pub completion_criteria: String,
    pub prohibited_scope: String,
    pub created_at_ms: i64,
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

/// Immutable evidence that the user granted one-time provider-transmission
/// consent for a specific `(task, provider, work_kind, task_version)`
/// combination. A new row is required whenever the task version changes
/// (e.g. after a resume); existing rows are never updated or reused across
/// versions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderConsent {
    pub task_id: TaskId,
    pub provider: ProviderKind,
    pub work_kind: WorkKind,
    pub approved_task_version: u64,
    pub consented_at_ms: i64,
}

/// Terminal classification of a Claude Planning attempt, already reduced
/// from the provider-specific streaming/exit-code/output-schema details to
/// the small vocabulary the Task state machine understands. `Completed` is
/// the only variant that carries a `plan_text`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningResultOutcome {
    Completed,
    Failed,
    Cancelled,
    RecoveryRequired,
}

/// Immutable, 1:1-per-task record of a Claude Planning attempt's safe final
/// result. `plan_text` is masked and size-bounded by the caller before this
/// record is built (see `chatoms_infrastructure::redaction::SecretRedactor`)
/// and is `Some` only when `outcome` is `Completed`; every other outcome
/// carries `None`. Never carries raw stdout/stderr, transcript, tool I/O,
/// login output, or an executable path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskPlanningResultRecord {
    pub task_id: TaskId,
    pub provider: ProviderKind,
    pub work_kind: WorkKind,
    pub outcome: PlanningResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub plan_text: Option<String>,
}

/// Terminal classification of a Claude Implementation attempt, already
/// reduced to the small vocabulary the Task state machine understands.
/// Unlike [`PlanningResultOutcome`], there is no `Failed` variant: because
/// Implementation can leave real, partial filesystem changes behind, a
/// nonzero exit, malformed output, an exceeded stdout bound, a timeout, or
/// an unconfirmed cancellation are never safe to classify as "failed and
/// discardable" — every such case is `RecoveryRequired` so a human reviews
/// the task worktree/Git diff before the outcome is treated as final. Only a
/// *confirmed* process exit (a clean successful envelope or a confirmed
/// cancellation) is not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImplementationResultOutcome {
    Completed,
    Cancelled,
    RecoveryRequired,
}

/// Immutable, 1:1-per-task record of a Claude Implementation attempt's safe
/// final result. Carries no content field (no transcript, stdout/stderr,
/// tool I/O, prompt, plan text, diff, executable path, or login/session
/// info): the actual change made by an Implementation attempt is recorded
/// nowhere but the task worktree/Git diff, which is its own source of
/// truth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskImplementationResultRecord {
    pub task_id: TaskId,
    pub provider: ProviderKind,
    pub work_kind: WorkKind,
    pub outcome: ImplementationResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

/// Terminal classification of a Claude Review attempt, already reduced from
/// the provider-specific streaming/exit-code/output-schema details to the
/// small vocabulary the Task state machine understands. Like
/// [`PlanningResultOutcome`] (Review is read-only — `--tools
/// Read,Glob,Grep` only — so a confirmed failure carries no risk of a
/// partial external effect), this has a `Failed` variant that
/// [`ImplementationResultOutcome`] deliberately does not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewResultOutcome {
    Completed,
    Failed,
    Cancelled,
    RecoveryRequired,
}

/// Immutable, 1:1-per-task record of a Claude Review attempt's safe final
/// result. `review_text` is masked and size-bounded by the caller before
/// this record is built (mirroring [`TaskPlanningResultRecord::plan_text`])
/// and is `Some` only when `outcome` is `Completed`; every other outcome
/// carries `None`. Never carries raw stdout/stderr, a raw Git diff,
/// transcript, tool I/O, prompt, executable/environment path, or
/// login/session/cost information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReviewResultRecord {
    pub task_id: TaskId,
    pub provider: ProviderKind,
    pub work_kind: WorkKind,
    pub outcome: ReviewResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub review_text: Option<String>,
}

/// Immutable, one-time-approved Testing validation command for
/// `(task_id, approved_task_version, kind)`. `executable`/`arguments` are
/// always exactly one candidate returned by
/// [`crate::validation::ValidationCommandDiscovery::discover_candidates`] at
/// approval time — the application layer must reject any request that does
/// not match a discovered candidate before this record is ever constructed.
/// A task version may have at most one approval per `kind`; approving the
/// same `kind` again for the same version is rejected as a duplicate.
///
/// `approved_executable_path`/`*_hex` bind that logical `executable` name to
/// the one specific file the user pointed at, plus the Windows stable NTFS
/// object identity (volume serial + file ID, via
/// [`crate::filesystem::FilesystemIdentityPort::inspect_supported_file`]/
/// `inspect_supported_directory`) of that file and its containing
/// directory. This is a deliberately **weaker trust model than Git or
/// Claude/Codex executable trust** (see `docs/DECISIONS.md`'s "Validation
/// tool executable trust" entry): there is no mandatory Authenticode signer
/// gate, because common Windows installs of these toolchains (e.g.
/// rustup-distributed `cargo`/`rustc`, npm/pnpm/yarn shims) are not reliably
/// signed by a single pinnable publisher the way the official Git for
/// Windows installer or the Claude Code CLI are. Path + stable file
/// identity, re-verified immediately before every future use, is the whole
/// trust basis; a mismatch must never be executed against and must require
/// a fresh approval, not an automatic repair. This Unit only stores and
/// read-only re-verifies the binding — it never executes the command.
///
/// `approved_cargo_home_path`/`cargo_home_*_hex` and
/// `approved_rustup_home_path`/`rustup_home_*_hex` optionally bind a
/// `CARGO_HOME`/`RUSTUP_HOME` environment directory the same way, each as an
/// all-or-nothing trio (`None` means the run has no approved override for
/// that variable, never that it is inherited from the calling process). Any
/// executor consuming this record must re-verify these bindings — not a
/// value it was separately constructed with — immediately before spawning,
/// exactly like the executable/tool-directory binding above.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationCommandApprovalRecord {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub kind: ValidationCommandKind,
    pub executable: String,
    pub arguments: Vec<String>,
    pub approved_executable_path: String,
    pub executable_volume_serial_hex: String,
    pub executable_file_id_hex: String,
    pub tool_directory_path: String,
    pub tool_directory_volume_serial_hex: String,
    pub tool_directory_file_id_hex: String,
    pub approved_cargo_home_path: Option<String>,
    pub cargo_home_volume_serial_hex: Option<String>,
    pub cargo_home_file_id_hex: Option<String>,
    pub approved_rustup_home_path: Option<String>,
    pub rustup_home_volume_serial_hex: Option<String>,
    pub rustup_home_file_id_hex: Option<String>,
    pub approved_at_ms: i64,
}

/// Terminal, fail-closed classification of one validation command attempt,
/// mirroring `chatoms_ports::validation_execution::ValidationExecutionOutcome`
/// one-to-one at the storage layer (a separate type, not a re-export: this
/// one is data-only — `ExitFailure` carries no inline `exit_code`, since that
/// value lives in [`ValidationCommandResultRecord::exit_code`] instead).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationCommandResultOutcome {
    Success,
    ExitFailure,
    TimedOut,
    StdoutBoundExceeded,
    Cancelled,
    Uncertain,
}

/// One attempt to run an already-approved
/// [`ValidationCommandApprovalRecord`], as a caller supplies it to
/// [`FoundationRepository::append_validation_command_result`]. Carries no
/// `attempt_sequence`: the repository is the sole owner of that value and
/// computes it atomically inside the same transaction that appends the row,
/// so a caller never has to guess or race for the next sequence number.
///
/// `safe_summary` must already be masked and size-bounded by the caller
/// before this value is built — this is not raw stdout/stderr and never
/// will be. A future orchestration Unit is responsible for producing it
/// (e.g. via `chatoms_infrastructure::redaction::SecretRedactor` plus its
/// own bound) from whatever `ValidationCommandExecutor` actually returned;
/// this type and its storage never re-derive, re-parse, or further redact
/// it, and never carry provider, session, transcript, executable path,
/// environment path, or raw stdout/stderr fields at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationCommandResultAttempt {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub kind: ValidationCommandKind,
    pub outcome: ValidationCommandResultOutcome,
    pub exit_code: Option<i32>,
    pub safe_summary: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

/// Immutable, append-only record of one attempt to run an approved
/// `task_validation_command_approvals` row. Unlike
/// [`TaskPlanningResultRecord`]/[`TaskImplementationResultRecord`] (one row
/// per task), Testing can re-enter through `AutoFixing`/`ReviewFixing` many
/// times, so a single approval may accumulate many attempts —
/// `attempt_sequence` orders them within `(task_id, approved_task_version,
/// kind)`, starting at 1.
///
/// Carries no content field beyond `safe_summary` (see
/// [`ValidationCommandResultAttempt`] for what that safety guarantee means)
/// and no provider/session/transcript/executable-path/environment-path
/// field at all — the command's executable/tool-directory/environment
/// identity is already recorded immutably on the approval row this result
/// is bound to; raw stdout/stderr are never stored anywhere by this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationCommandResultRecord {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub kind: ValidationCommandKind,
    pub attempt_sequence: u32,
    pub outcome: ValidationCommandResultOutcome,
    pub exit_code: Option<i32>,
    pub safe_summary: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppProfileRecord {
    pub id: String,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderBindingRecord {
    pub id: String,
    pub app_profile_id: String,
    pub provider_kind: ProviderKind,
    pub display_name: String,
    pub executable_path: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
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
        _brief: Option<&TaskBriefRecord>,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_task_isolation(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_task_brief(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskBriefRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Reads back the immutable, already-safe Claude Planning result row for
    /// `task_id` (see [`TaskPlanningResultRecord`] for the safety
    /// guarantees). Returns `None` when no attempt has been recorded yet.
    /// Never re-derives or re-parses provider output — this is a read of
    /// exactly what [`Self::save_planning_result`] persisted.
    fn get_task_planning_result(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskPlanningResultRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Reads back the immutable, already-safe Claude Implementation result
    /// row for `task_id` (see [`TaskImplementationResultRecord`] for the
    /// safety guarantees). Returns `None` when no attempt has been recorded
    /// yet. Never re-derives or re-parses provider output — this is a read
    /// of exactly what [`Self::save_implementation_result`] persisted.
    fn get_task_implementation_result(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskImplementationResultRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Reads back the immutable, already-safe Claude Review result row for
    /// `task_id` (see [`TaskReviewResultRecord`] for the safety guarantees).
    /// Returns `None` when no attempt has been recorded yet. Never
    /// re-derives or re-parses provider output — this is a read of exactly
    /// what [`Self::save_review_result`] persisted.
    fn get_task_review_result(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskReviewResultRecord>, RepositoryError> {
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

    fn get_provider_consent(
        &mut self,
        _task_id: TaskId,
        _provider: ProviderKind,
        _work_kind: WorkKind,
        _approved_task_version: u64,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists the `WorktreeReady -> Planning` state update and
    /// its transition history record, along with `consent` when a new
    /// consent grant must be recorded. Pass `None` when an already-valid
    /// consent for the same task version is being reused: no consent row is
    /// written, but the state/history write remains atomic on its own.
    fn save_planning_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists the `AwaitingDesignApproval -> Implementing` state
    /// update and its transition history record, along with `consent` when a
    /// new consent grant must be recorded. Pass `None` when an already-valid
    /// consent for the same task version is being reused: no consent row is
    /// written, but the state/history write remains atomic on its own.
    fn save_implementation_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically records or reuses a one-time Claude Review
    /// provider-transmission consent for `(task_id, Claude, Review,
    /// expected_version)`. Unlike [`Self::save_planning_transition`]/
    /// [`Self::save_implementation_transition`], this never touches task
    /// state, version, transition history, or the `ActiveTaskLease`:
    /// `Testing -> Reviewing` is already an automatic transition (see
    /// `TaskService::finalize_validation_command_batch`), so by the time a
    /// Review consent can be granted the task is already `Reviewing` and
    /// stays there. Implementations must re-verify inside the same
    /// transaction that the task's current version equals `expected_version`
    /// and its current state is `Reviewing` before reading or inserting the
    /// consent row, and must return an existing same-version consent
    /// unchanged rather than inserting a duplicate.
    fn save_review_consent(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _consented_at_ms: i64,
    ) -> Result<ProviderConsent, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists a Claude Planning attempt's safe final result,
    /// the resulting state update away from `Planning`, and its transition
    /// history record. `terminal` must equal `task.state().is_terminal()`;
    /// implementations cross-check this and release the `ActiveTaskLease`
    /// only when it is `true` (matching `terminate_task`'s lease handling).
    fn save_planning_result(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _result: &TaskPlanningResultRecord,
        _terminal: bool,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists a Claude Implementation attempt's safe final
    /// result, the resulting state update away from `Implementing`
    /// (`Completed -> Testing`, confirmed `Cancelled -> Paused`, every other
    /// outcome -> `RecoveryRequired`), and its transition history record.
    /// None of these target states is terminal, so unlike
    /// [`Self::save_planning_result`] this never releases the
    /// `ActiveTaskLease`.
    fn save_implementation_result(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _result: &TaskImplementationResultRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists a Claude Review attempt's safe final result, the
    /// resulting state update away from `Reviewing` (`Completed ->
    /// AwaitingUserDiffApproval`, `Failed -> Failed`, confirmed `Cancelled ->
    /// Paused` with `resume_target_state = Reviewing`, `RecoveryRequired ->
    /// RecoveryRequired`), and its transition history record. `terminal` must
    /// equal `task.state().is_terminal()` (`true` only for `Failed`, the
    /// only terminal outcome among these four); implementations cross-check
    /// this and release the `ActiveTaskLease` only when it is `true`,
    /// matching [`Self::save_planning_result`]'s contract.
    fn save_review_result(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _result: &TaskReviewResultRecord,
        _terminal: bool,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists a one-time-approved Testing validation command.
    /// Implementations must re-verify inside the same transaction that the
    /// task's current version equals `approval.approved_task_version` and
    /// its current state is `Implementing` or `Testing` before inserting;
    /// the row itself is immutable once written and rejects a second
    /// approval for the same `(task_id, approved_task_version, kind)`. This
    /// Unit never executes the approved command — see
    /// [`crate::validation::ValidationCommandDiscovery`] for the read-only
    /// discovery step a caller must run first.
    fn save_validation_command_approval(
        &mut self,
        _approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Reads back every immutable validation command approval recorded for
    /// `(task_id, approved_task_version)`. Never re-derives or re-validates —
    /// a pure read of exactly what
    /// [`Self::save_validation_command_approval`] persisted.
    fn list_validation_command_approvals(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically appends one immutable validation command result attempt
    /// for an already-approved `(task_id, approved_task_version, kind)`.
    /// Implementations must, inside a single `IMMEDIATE` transaction: verify
    /// the approval row actually exists (defense-in-depth beyond the SQL
    /// foreign key), atomically compute the next `attempt_sequence` for that
    /// approval, and insert the row with that computed sequence — a caller
    /// never supplies or guesses it. Never validates or changes `Task`
    /// state or version: that is a later orchestration Unit's
    /// responsibility, not this append-only storage boundary's.
    fn append_validation_command_result(
        &mut self,
        _attempt: &ValidationCommandResultAttempt,
    ) -> Result<ValidationCommandResultRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Reads back every immutable validation command result attempt
    /// recorded for `(task_id, approved_task_version, kind)`, ordered by
    /// `attempt_sequence`. Never re-derives or re-parses — a pure read of
    /// exactly what [`Self::append_validation_command_result`] persisted.
    fn list_validation_command_results(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
        _kind: ValidationCommandKind,
    ) -> Result<Vec<ValidationCommandResultRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically appends the final validation command result attempt for a
    /// Testing batch and applies the resulting `Testing ->
    /// Reviewing/Paused/RecoveryRequired` state transition and history
    /// entry in the same transaction — mirroring
    /// [`Self::save_implementation_result`]'s "result row + transition, one
    /// transaction" shape, but scoped to the one *final* attempt of a batch
    /// (see `crate::validation_execution` and
    /// `chatoms_application::testing_execution` for why only the last
    /// attempt needs this: every earlier `Success` attempt is a plain
    /// [`Self::append_validation_command_result`] with no state change).
    /// Implementations must verify the approval this attempt is bound to
    /// actually exists and compute its `attempt_sequence` atomically, the
    /// same as `append_validation_command_result`. Never validates or
    /// changes task version beyond the one transition supplied.
    fn finalize_validation_command_batch(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _attempt: &ValidationCommandResultAttempt,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn ensure_default_profile_and_claude_binding(
        &mut self,
        _profile: &AppProfileRecord,
        _binding: &ProviderBindingRecord,
    ) -> Result<ProviderBindingRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_claude_binding(
        &mut self,
        _profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn update_claude_executable_path(
        &mut self,
        _binding_id: &str,
        _executable_path: Option<&str>,
        _updated_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }
}
