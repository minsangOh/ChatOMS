use std::{error::Error, fmt};

use chatoms_domain::{
    ContextDataScope, GitOperationId, HighRiskCategory, ProjectId, Task, TaskId,
    TaskStateTransition, ValidationCommandKind, ValidationExecutionScope, WorkKind,
};

use crate::diff::DiffContentHash;
use crate::git::RepositoryKind;
use crate::manual_merge_resolution::ManualResolutionDigest;
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
/// consent for a specific `(task, provider, work_kind, task_version,
/// data_scope)` combination. `data_scope` is a fifth identity component,
/// distinct from `work_kind`: it identifies *what data* the consent covers,
/// not *which work kind* the provider is performing. A new row is required
/// whenever the task version changes (e.g. after a resume) or the data
/// scope changes; existing rows are never updated or reused across either
/// dimension. See [`ContextDataScope`] for the fixed vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderConsent {
    pub task_id: TaskId,
    pub provider: ProviderKind,
    pub work_kind: WorkKind,
    pub approved_task_version: u64,
    pub data_scope: ContextDataScope,
    pub consented_at_ms: i64,
}

/// Immutable, content-free identity record proving a Context Package v1
/// manifest exists for a specific `(task_id, provider, work_kind,
/// approved_task_version, data_scope)` provider-transmission consent (see
/// [`ProviderConsent`]). `data_scope` is always
/// [`ContextDataScope::ContextPackageV1`] — a manifest never exists for a
/// [`ContextDataScope::LegacyPhase4`] consent, which has no manifest
/// concept. Alternative B (`docs/DECISIONS.md`, "Context Package v1 저장
/// 방식") keeps this record permanently as proof that a package was built
/// for this exact consent; the actual assembled body (TaskBrief text,
/// plan/review/validation content, Git diff, file/symbol references, or any
/// other content) is never stored here or anywhere else — it exists only
/// momentarily, immediately before a provider call, and is discarded
/// afterward. This type therefore carries no content field beyond the
/// consent identity it refers to and the time the row was written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextPackageManifestRecord {
    pub task_id: TaskId,
    pub provider: ProviderKind,
    pub work_kind: WorkKind,
    pub approved_task_version: u64,
    pub data_scope: ContextDataScope,
    pub created_at_ms: i64,
}

/// The exact `ContextPackageV1` consent and its FK-bound manifest, returned
/// together because they are always created or reused together — never one
/// without the other (see [`FoundationRepository::prepare_planning_context_package`]
/// and its Implementation/Review siblings). Both fields are already
/// content-free ([`ProviderConsent`]/[`ContextPackageManifestRecord`]), so
/// this type adds no new content of its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextPackagePreparation {
    pub consent: ProviderConsent,
    pub manifest: ContextPackageManifestRecord,
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
    pub execution_scope: ValidationExecutionScope,
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
    pub target_project_id: Option<ProjectId>,
    pub target_project_identity_revision: Option<u64>,
    pub target_root_volume_serial_hex: Option<String>,
    pub target_root_file_id_hex: Option<String>,
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
    pub execution_scope: ValidationExecutionScope,
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
    pub execution_scope: ValidationExecutionScope,
    pub kind: ValidationCommandKind,
    pub attempt_sequence: u32,
    pub outcome: ValidationCommandResultOutcome,
    pub exit_code: Option<i32>,
    pub safe_summary: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostMergeValidationResultOutcome {
    Success,
    ExitFailure,
    TimedOut,
    StdoutBoundExceeded,
    BindingRejected,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostMergeValidationResultAttempt {
    pub task_id: TaskId,
    pub approval_task_version: u64,
    pub post_merge_task_version: u64,
    pub execution_scope: ValidationExecutionScope,
    pub kind: ValidationCommandKind,
    pub outcome: PostMergeValidationResultOutcome,
    pub exit_code: Option<i32>,
    pub safe_summary: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostMergeValidationResultRecord {
    pub task_id: TaskId,
    pub approval_task_version: u64,
    pub post_merge_task_version: u64,
    pub execution_scope: ValidationExecutionScope,
    pub kind: ValidationCommandKind,
    pub attempt_sequence: u32,
    pub outcome: PostMergeValidationResultOutcome,
    pub exit_code: Option<i32>,
    pub safe_summary: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

/// An immutable, content-free approval of one [`HighRiskCategory`] effect for
/// a specific task version. Unlike [`ValidationCommandApprovalRecord`] or a
/// [`ProviderConsent`], this record carries no `provider`, `work_kind`, or
/// `data_scope` at all: whether an operation's effect falls into a high-risk
/// category (a schema change, a data migration, a difficult-to-recover
/// change, and so on) is orthogonal to which provider or work kind performs
/// it. This Unit only stores and reads back this reference — it never
/// classifies an operation into a category, never gates any execution on
/// its presence, and is not foreign-keyed to `task_provider_consents` or
/// `context_package_manifests`, since a task may need both a data-scope
/// consent and a high-risk approval for the same change at the same time,
/// entirely independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HighRiskApprovalRecord {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub risk_category: HighRiskCategory,
    pub approved_at_ms: i64,
}

/// An immutable, content-free approval binding a specific task version to
/// the exact [`DiffContentHash`] of the worktree diff the user reviewed and
/// approved — never the raw diff text itself. Unlike [`HighRiskApprovalRecord`]
/// (which is keyed by a closed 13-item category vocabulary), the diff this
/// approval covers has no prior commit to bind to at approval time (the
/// single work commit is only created once `Merging` starts — see
/// `docs/DECISIONS.md`'s "병합 이력" entry): the content hash is the only
/// content-free way to prove "the user approved *this exact* diff, not a
/// different one at the same task version." Not foreign-keyed to
/// `task_provider_consents`, `context_package_manifests`, or
/// `task_high_risk_approvals` — this is an entirely independent approval
/// axis. This Unit only stores and reads back this reference — it never
/// starts, blocks, or gates any provider, `AutoFixing`, `ReviewFixing`, or
/// `Merging` execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiffApprovalRecord {
    pub task_id: TaskId,
    pub approved_task_version: u64,
    pub diff_content_hash: DiffContentHash,
    pub approved_at_ms: i64,
}

/// Immutable, content-free confirmation that a user reviewed and approved
/// the exact staged index a manual `MergeConflict` resolution left in the
/// original checkout, for one `(task_id, merge_conflict_task_version,
/// resolution_digest)` identity. Distinct from [`DiffApprovalRecord`] (which
/// binds to the task diff reviewed *before* `Merging` starts) — this
/// approval only exists once a real `MergeConflict` requires a human
/// decision, and it is never foreign-keyed to `task_diff_approvals`,
/// `task_provider_consents`, `context_package_manifests`, or
/// `task_high_risk_approvals`. Never carries a raw path, file content, or
/// Git stdout/stderr — only commit identity and the digest itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualMergeResolutionConfirmationRecord {
    pub task_id: TaskId,
    pub merge_conflict_task_version: u64,
    pub source_approval_task_version: u64,
    pub base_commit: String,
    pub task_commit: String,
    pub merge_head_commit: String,
    pub resolution_digest: ManualResolutionDigest,
    pub confirmed_at_ms: i64,
}

/// Immutable, content-free approval that a user explicitly approved
/// aborting one task's in-progress `MergeConflict` merge, for one
/// `(task_id, merge_conflict_task_version)` identity. A distinct approval
/// axis from [`ManualMergeResolutionConfirmationRecord`]: that confirmation
/// approves *continuing* a specific staged resolution and binds to its
/// resolution digest, while this approval discards it entirely and
/// deliberately does not bind to any resolution digest. Never foreign-keyed
/// to `task_diff_approvals`, `task_provider_consents`,
/// `context_package_manifests`, `task_high_risk_approvals`, or
/// `task_manual_merge_resolution_confirmations` — a task may need any
/// combination of these entirely independently. Never carries a raw path,
/// file content, or Git stdout/stderr — only commit identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeAbortApprovalRecord {
    pub task_id: TaskId,
    pub merge_conflict_task_version: u64,
    pub source_approval_task_version: u64,
    pub base_commit: String,
    pub task_commit: String,
    pub merge_head_commit: String,
    pub approved_at_ms: i64,
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

    /// Looks up a consent for the exact `(task_id, provider, work_kind,
    /// approved_task_version, data_scope)` 5-tuple. Implementations must
    /// never omit `data_scope` from the lookup, fall back to a different
    /// scope, or treat a consent recorded under one scope as valid for
    /// another: a caller asking for `ContextDataScope::ContextPackageV1`
    /// must never receive a `ContextDataScope::LegacyPhase4` row, and vice
    /// versa.
    fn get_provider_consent(
        &mut self,
        _task_id: TaskId,
        _provider: ProviderKind,
        _work_kind: WorkKind,
        _approved_task_version: u64,
        _data_scope: ContextDataScope,
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
    /// expected_version, data_scope)`. Unlike [`Self::save_planning_transition`]/
    /// [`Self::save_implementation_transition`], this never touches task
    /// state, version, transition history, or the `ActiveTaskLease`:
    /// `Testing -> Reviewing` is already an automatic transition (see
    /// `TaskService::finalize_validation_command_batch`), so by the time a
    /// Review consent can be granted the task is already `Reviewing` and
    /// stays there. Implementations must re-verify inside the same
    /// transaction that the task's current version equals `expected_version`
    /// and its current state is `Reviewing` before reading or inserting the
    /// consent row, and must return an existing same-version-and-scope
    /// consent unchanged rather than inserting a duplicate. A consent
    /// recorded under a different `data_scope` must never be returned or
    /// reused as if it matched.
    fn save_review_consent(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _data_scope: ContextDataScope,
        _consented_at_ms: i64,
    ) -> Result<ProviderConsent, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically commits the `WorktreeReady -> Planning` state update and its
    /// transition history record for a Context Package v1 Planning
    /// activation, requiring — and re-verifying inside the same `IMMEDIATE`
    /// transaction — that the exact `(task_id, Claude, Planning,
    /// expected_version, ContextPackageV1)` consent and its FK-bound manifest
    /// (see [`Self::prepare_planning_context_package`]) already exist. Unlike
    /// [`Self::save_planning_transition`], this never inserts a new consent
    /// row of its own — the [`ContextDataScope::ContextPackageV1`] consent
    /// must already have been prepared by a prior, separate call — and it
    /// never shares a write path with `save_planning_transition` or
    /// `prepare_planning_context_package`. If the consent/manifest pair is
    /// entirely absent, this is a normal (not corrupted) precondition
    /// failure: the caller simply has not prepared the package yet, and this
    /// returns `RepositoryErrorCode::InvalidAggregate` without writing
    /// anything. If exactly one of the pair exists, that is the same
    /// already-corrupted invariant `prepare_planning_context_package`
    /// guards against, and this returns
    /// `RepositoryErrorCode::InvalidPersistenceState` without writing
    /// anything. Never touches a [`ContextDataScope::LegacyPhase4`] consent
    /// under any circumstance.
    fn save_context_package_planning_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically commits the `AwaitingDesignApproval -> Implementing` state
    /// update and its transition history record for a Context Package v1
    /// Implementation activation, requiring — and re-verifying inside the
    /// same `IMMEDIATE` transaction — that the exact `(task_id, Claude,
    /// Implementation, expected_version, ContextPackageV1)` consent and its
    /// FK-bound manifest already exist, *and* that a `Completed`
    /// [`crate::repository::TaskPlanningResultRecord`] with non-empty
    /// `plan_text` is already stored for this task. Mirrors
    /// [`Self::save_context_package_planning_transition`], this never
    /// inserts a new consent row of its own — the
    /// [`ContextDataScope::ContextPackageV1`] consent must already have been
    /// prepared by a prior, separate call — and it never shares a write path
    /// with `save_implementation_transition` or
    /// `prepare_implementation_context_package`. If the consent/manifest
    /// pair is entirely absent, this is a normal (not corrupted)
    /// precondition failure: the caller simply has not prepared the package
    /// yet, and this returns `RepositoryErrorCode::InvalidAggregate` without
    /// writing anything. If exactly one of the pair exists, or if the stored
    /// Planning result is missing, not `Completed`, or has empty
    /// `plan_text`, that is an already-corrupted invariant and this returns
    /// `RepositoryErrorCode::InvalidPersistenceState` without writing
    /// anything. Never touches a [`ContextDataScope::LegacyPhase4`] consent
    /// under any circumstance.
    fn save_context_package_implementation_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates or reuses the exact `(task_id, Claude, Planning,
    /// expected_version, ContextPackageV1)` consent together with its
    /// FK-bound manifest, in a single transaction — never one without the
    /// other. Requires `task.state() == WorktreeReady` at `expected_version`
    /// *and* a `WorktreeReady` isolation record; neither is relaxed for this
    /// method. Never touches task state, version, transition history, or
    /// the `ActiveTaskLease` — this is a pure consent/manifest preparation
    /// boundary, not a state transition (unlike
    /// [`Self::save_planning_transition`], which this method does not call,
    /// extend, or share a write path with). Does not create or reuse a
    /// [`ContextDataScope::LegacyPhase4`] consent under any circumstance.
    ///
    /// If exactly one of the consent/manifest pair already exists for this
    /// identity, that is an already-corrupted invariant this method must
    /// never silently repair: it returns
    /// `RepositoryErrorCode::InvalidPersistenceState` and writes nothing.
    fn prepare_planning_context_package(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates or reuses the exact `(task_id, Claude,
    /// Implementation, expected_version, ContextPackageV1)` consent together
    /// with its FK-bound manifest, in a single transaction. Requires
    /// `task.state() == AwaitingDesignApproval` at `expected_version`. See
    /// [`Self::prepare_planning_context_package`] for the shared contract
    /// (no state/version/history/lease mutation, no `LegacyPhase4` reuse, no
    /// silent repair of a partial consent/manifest pair).
    fn prepare_implementation_context_package(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates or reuses the exact `(task_id, Claude, Review,
    /// expected_version, ContextPackageV1)` consent together with its
    /// FK-bound manifest, in a single transaction. Requires
    /// `task.state() == Reviewing` at `expected_version` — mirroring
    /// [`Self::save_review_consent`]'s existing no-transition shape, but
    /// this method never calls, extends, or shares a write path with
    /// `save_review_consent`. See [`Self::prepare_planning_context_package`]
    /// for the shared contract.
    fn prepare_review_context_package(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically persists an immutable Context Package v1 manifest for the
    /// exact `(task_id, provider, work_kind, approved_task_version,
    /// data_scope)` consent `record.data_scope` identifies. The SQL foreign
    /// key (`0017_context_package_manifests.sql`) requires the matching
    /// [`ContextDataScope::ContextPackageV1`] consent row to already exist,
    /// so this can never succeed ahead of (or in place of) a real consent
    /// grant. Implementations must never widen, ignore, or substitute the
    /// requested scope for another one, and must never treat a duplicate
    /// identity, a missing consent, or a task version mismatch as success.
    /// Never validates or changes `Task` state, version, transition history,
    /// or the `ActiveTaskLease` — this is a pure storage boundary alongside
    /// an already-recorded consent, not a state transition.
    fn save_context_package_manifest(
        &mut self,
        _record: &ContextPackageManifestRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Looks up the Context Package v1 manifest for the exact `(task_id,
    /// provider, work_kind, approved_task_version, data_scope)` 5-tuple.
    /// Implementations must never omit `data_scope` from the lookup, fall
    /// back to a different scope, or return a manifest recorded under one
    /// scope as if it matched another. Returns `None` when no manifest has
    /// been recorded for that exact identity.
    fn get_context_package_manifest(
        &mut self,
        _task_id: TaskId,
        _provider: ProviderKind,
        _work_kind: WorkKind,
        _approved_task_version: u64,
        _data_scope: ContextDataScope,
    ) -> Result<Option<ContextPackageManifestRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Immutably inserts one [`HighRiskApprovalRecord`]. The SQL trigger
    /// (`0018_task_high_risk_approvals.sql`) enforces that
    /// `approved_task_version` equals the task's current version at insert
    /// time; a duplicate `(task_id, approved_task_version, risk_category)`
    /// is rejected by the table's primary key. This Unit performs no
    /// create-or-reuse logic and no task state check — a duplicate or a
    /// stale version must surface as a repository error, never be silently
    /// treated as success. Never validates or changes `Task` state,
    /// transition history, or the `ActiveTaskLease`.
    fn save_high_risk_approval(
        &mut self,
        _approval: &HighRiskApprovalRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Looks up the high-risk approval for the exact `(task_id,
    /// approved_task_version, risk_category)` identity. Returns `None` when
    /// no approval has been recorded for that exact identity — never
    /// falls back to a different version or category. An unrecognized
    /// persisted `risk_category` (a corrupted or hand-edited row) fails
    /// closed as a typed persistence error rather than exposing the raw
    /// stored value or silently defaulting to `None`.
    fn get_high_risk_approval(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
        _risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates-or-reuses one immutable [`HighRiskApprovalRecord`]
    /// for the exact `(task_id, expected_version, risk_category)` identity,
    /// inside a single `IMMEDIATE` transaction: re-verify the task exists and
    /// its current version equals `expected_version`, look up the exact
    /// existing approval, and either return it unchanged or insert a new one
    /// and return that — never both, and never a duplicate row. Unlike
    /// [`Self::save_high_risk_approval`] (a bare immutable insert that
    /// surfaces a duplicate identity as an error), this method treats an
    /// already-existing exact match as success, not a conflict — the
    /// `IMMEDIATE` lock this transaction acquires up front is what makes
    /// that reuse race-free against a concurrent caller doing the same
    /// thing, exactly like [`Self::save_review_consent`]'s create-or-reuse
    /// shape. Does not whitelist or validate the task's current *state* —
    /// which state(s) require which category is a future Policy Engine's
    /// responsibility, not this storage boundary's. Never validates or
    /// changes `Task` state, transition history, the `ActiveTaskLease`, or
    /// any provider consent/manifest/validation-approval row.
    fn ensure_high_risk_approval(
        &mut self,
        _task_id: TaskId,
        _expected_version: u64,
        _risk_category: HighRiskCategory,
        _approved_at_ms: i64,
    ) -> Result<HighRiskApprovalRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Immutably inserts one [`DiffApprovalRecord`]. The SQL trigger
    /// (`0019_task_diff_approvals.sql`) enforces that
    /// `approved_task_version` equals the task's current version at insert
    /// time; a duplicate `(task_id, approved_task_version,
    /// diff_content_hash)` is rejected by the table's primary key. This
    /// method performs no create-or-reuse logic and no task state check — a
    /// duplicate or a stale version must surface as a repository error,
    /// never be silently treated as success. Never validates or changes
    /// `Task` state, transition history, or the `ActiveTaskLease`.
    fn save_diff_approval(
        &mut self,
        _approval: &DiffApprovalRecord,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Looks up the diff approval for the exact `(task_id,
    /// approved_task_version, diff_content_hash)` identity. Returns `None`
    /// when no approval has been recorded for that exact identity — never
    /// falls back to a different version or hash. A malformed persisted hex
    /// digest (a corrupted or hand-edited row) fails closed as a typed
    /// persistence error rather than exposing the raw stored value or
    /// silently defaulting to `None`.
    fn get_diff_approval(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
        _diff_content_hash: DiffContentHash,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_diff_approval_for_task_version(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates-or-reuses one immutable [`DiffApprovalRecord`] for
    /// the exact `(task_id, expected_version, diff_content_hash)` identity,
    /// inside a single `IMMEDIATE` transaction: re-verify the task exists
    /// and its current version equals `expected_version`, look up the exact
    /// existing approval, and either return it unchanged or insert a new
    /// one and return that — never both, and never a duplicate row. Mirrors
    /// [`Self::ensure_high_risk_approval`]'s create-or-reuse shape. Callers
    /// are responsible for recomputing `diff_content_hash` from the task's
    /// *current* worktree diff and verifying it exactly matches whatever
    /// hash the user was shown before calling this — this method itself
    /// does not read or re-verify any diff. Does not validate the task's
    /// current *state*: gating which state(s) may call this remains the
    /// caller's responsibility (see
    /// `chatoms_application::user_diff_approval`). Never validates or
    /// changes `Task` state, transition history, the `ActiveTaskLease`, or
    /// any provider consent/manifest/high-risk-approval row.
    fn ensure_diff_approval(
        &mut self,
        _task_id: TaskId,
        _expected_version: u64,
        _diff_content_hash: DiffContentHash,
        _approved_at_ms: i64,
    ) -> Result<DiffApprovalRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Looks up the immutable manual-resolution confirmation for the exact
    /// `(task_id, merge_conflict_task_version, resolution_digest)` identity.
    /// Returns `None` when no confirmation has been recorded for that exact
    /// identity — never falls back to a different version or digest. A
    /// malformed persisted commit hash or digest (a corrupted or
    /// hand-edited row) fails closed as a typed persistence error rather
    /// than exposing the raw stored value or silently defaulting to `None`.
    fn get_manual_merge_resolution_confirmation(
        &mut self,
        _task_id: TaskId,
        _merge_conflict_task_version: u64,
        _resolution_digest: ManualResolutionDigest,
    ) -> Result<Option<ManualMergeResolutionConfirmationRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates-or-reuses one immutable
    /// [`ManualMergeResolutionConfirmationRecord`] for the exact `(task_id,
    /// merge_conflict_task_version, resolution_digest)` identity, inside a
    /// single `IMMEDIATE` transaction: re-verify the task exists and its
    /// current version and state equal `merge_conflict_task_version` and
    /// `MergeConflict`, look up the exact existing confirmation, and either
    /// return it unchanged or insert a new one and return that — never
    /// both, and never a duplicate row. A different `resolution_digest` for
    /// the same `(task_id, merge_conflict_task_version)` is always a
    /// separate immutable row; an earlier confirmation is never updated or
    /// deleted. Never validates or changes `Task` state, transition
    /// history, the `ActiveTaskLease`, or any other approval/consent row.
    #[allow(clippy::too_many_arguments)]
    fn ensure_manual_merge_resolution_confirmation(
        &mut self,
        _task_id: TaskId,
        _merge_conflict_task_version: u64,
        _source_approval_task_version: u64,
        _base_commit: &str,
        _task_commit: &str,
        _merge_head_commit: &str,
        _resolution_digest: ManualResolutionDigest,
        _confirmed_at_ms: i64,
    ) -> Result<ManualMergeResolutionConfirmationRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically commits the `MergeConflict -> Merging` state update and
    /// its transition history record, requiring — and re-verifying inside
    /// the same `IMMEDIATE` transaction — that an exact
    /// `(task_id, expected_version, resolution_digest)` manual-resolution
    /// confirmation already exists (see
    /// [`Self::ensure_manual_merge_resolution_confirmation`]). If it is
    /// absent, this is a normal (not corrupted) precondition failure: the
    /// caller has not confirmed a resolution for the *current* staged index
    /// yet, and this returns `RepositoryErrorCode::InvalidAggregate`
    /// without writing anything. Never inserts a confirmation row of its
    /// own and never shares a write path with
    /// [`Self::ensure_manual_merge_resolution_confirmation`].
    fn save_manual_merge_resolution_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _resolution_digest: ManualResolutionDigest,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Looks up the immutable merge-abort approval for the exact
    /// `(task_id, merge_conflict_task_version)` identity. Returns `None`
    /// when no approval has been recorded for that exact identity — never
    /// falls back to a different version. A malformed persisted commit hash
    /// (a corrupted or hand-edited row) fails closed as a typed persistence
    /// error rather than exposing the raw stored value or silently
    /// defaulting to `None`.
    fn get_merge_abort_approval(
        &mut self,
        _task_id: TaskId,
        _merge_conflict_task_version: u64,
    ) -> Result<Option<MergeAbortApprovalRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically creates-or-reuses one immutable [`MergeAbortApprovalRecord`]
    /// for the exact `(task_id, merge_conflict_task_version)` identity,
    /// inside a single `IMMEDIATE` transaction: re-verify the task exists
    /// and its current version and state equal
    /// `merge_conflict_task_version` and `MergeConflict`, look up the exact
    /// existing approval, and either return it unchanged or insert a new
    /// one and return that — never both, and never a duplicate row. Unlike
    /// [`Self::ensure_manual_merge_resolution_confirmation`], approval
    /// identity does not include a resolution digest, so this is the only
    /// approval row this `(task_id, merge_conflict_task_version)` pair can
    /// ever have; a stored `base_commit`/`task_commit`/`merge_head_commit`
    /// that disagrees with the caller's request is a mismatch against a
    /// different merge identity and is rejected rather than reused. Never
    /// validates or changes `Task` state, transition history, the
    /// `ActiveTaskLease`, or any other approval/consent row.
    #[allow(clippy::too_many_arguments)]
    fn ensure_merge_abort_approval(
        &mut self,
        _task_id: TaskId,
        _merge_conflict_task_version: u64,
        _source_approval_task_version: u64,
        _base_commit: &str,
        _task_commit: &str,
        _merge_head_commit: &str,
        _approved_at_ms: i64,
    ) -> Result<MergeAbortApprovalRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    /// Atomically commits the `MergeConflict -> Cancelled` state update, its
    /// transition history record, and — because `Cancelled` is terminal —
    /// releases the `ActiveTaskLease`, requiring — and re-verifying inside
    /// the same `IMMEDIATE` transaction — that an exact `(task_id,
    /// expected_version)` merge-abort approval already exists (see
    /// [`Self::ensure_merge_abort_approval`]). If it is absent, this is a
    /// normal (not corrupted) precondition failure: the caller has not
    /// approved aborting this exact `MergeConflict` occurrence yet, and this
    /// returns `RepositoryErrorCode::InvalidAggregate` without writing
    /// anything. `terminal` must equal `task.state().is_terminal()`
    /// (`true` for the only outcome this method supports, `Cancelled`).
    /// Never inserts an approval row of its own and never shares a write
    /// path with [`Self::ensure_merge_abort_approval`].
    fn save_merge_abort_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _terminal: bool,
    ) -> Result<(), RepositoryError> {
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

    fn list_validation_command_approvals_for_scope(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
        _execution_scope: ValidationExecutionScope,
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

    fn append_post_merge_validation_result(
        &mut self,
        _attempt: &PostMergeValidationResultAttempt,
    ) -> Result<PostMergeValidationResultRecord, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn list_post_merge_validation_results(
        &mut self,
        _task_id: TaskId,
        _approval_task_version: u64,
        _post_merge_task_version: u64,
        _kind: ValidationCommandKind,
    ) -> Result<Vec<PostMergeValidationResultRecord>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn finalize_post_merge_validation_batch(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
        _attempt: &PostMergeValidationResultAttempt,
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
