import { isRecord } from "./errors";

export type HealthState = "healthy" | "degraded" | "unavailable";
export type StorageStatus = "ready" | "unavailable" | "insecure" | "unsupported";
export type DatabaseStatus =
  | "notChecked"
  | "ready"
  | "upgraded"
  | "migrationRequired"
  | "unavailable"
  | "incompatible";
export type LoggingStatus = "notChecked" | "ready" | "unavailable";
export type ActiveTaskStatusKind = "notChecked" | "none" | "active";
export type CapabilityStatus = "supported" | "unsupported" | "unavailable";
export type IpcSeverity = "info" | "warning" | "error" | "critical";
export type IpcRetry = "never" | "immediate" | "afterUserAction" | "afterStateRefresh";

export type TaskState =
  | "created"
  | "projectValidated"
  | "awaitingGitInitApproval"
  | "gitInitialized"
  | "worktreeCreating"
  | "worktreeReady"
  | "planning"
  | "awaitingDesignApproval"
  | "implementing"
  | "testing"
  | "autoFixing"
  | "reviewing"
  | "reviewFixing"
  | "awaitingUserDiffApproval"
  | "merging"
  | "mergeConflict"
  | "postMergeTesting"
  | "completed"
  | "paused"
  | "failed"
  | "recoveryRequired"
  | "unknownExternalEffect"
  | "cancelled"
  | "cleanupPending"
  | "archived";

export type MergeConflictInspectionOutcome =
  | "confirmedUnresolved"
  | "resolvedPendingConfirmation"
  | "restoredPendingAbortConfirmation"
  | "inconsistent"
  | "unavailable";

export type MergeConflictCountsDto = {
  readonly total: number;
  readonly bothModified: number;
  readonly bothAdded: number;
  readonly bothDeleted: number;
  readonly addedByUs: number;
  readonly addedByThem: number;
  readonly deletedByUs: number;
  readonly deletedByThem: number;
};

export type MergeConflictInspectionDto = {
  readonly outcome: MergeConflictInspectionOutcome;
  readonly counts: MergeConflictCountsDto;
};

export type WorkKind = "planning" | "implementation" | "review";
export type ProviderKind = "claude" | "codex";
export type ContractStatus = "approved" | "notApproved";
export type EligibilityBlockingReason =
  | "capabilityUnavailable"
  | "capabilityUnsupported"
  | "contractNotApproved"
  | "taskStateMismatch";

export type ProviderEligibilityDto = {
  readonly workKind: WorkKind;
  readonly provider: ProviderKind;
  readonly capability: CapabilityStatus;
  readonly contract: ContractStatus;
  readonly eligible: boolean;
  readonly stateAllowsWorkKind: boolean;
  readonly blockingReasons: readonly EligibilityBlockingReason[];
};

/// Fixed, single-value data-scope vocabulary. Always exactly
/// `"contextPackageV1"` — there is no other value this type accepts, and
/// `isContextPackagePreparationDto` below rejects anything else rather than
/// coercing or defaulting it.
export type ContextPackageDataScope = "contextPackageV1";

/// Content-free confirmation that a `ContextPackageV1` consent and its
/// FK-bound manifest now exist for a task (created or reused — the two are
/// indistinguishable by design). Never carries a `taskId`, raw TaskBrief
/// text, plan text, diff, validation summary, assembled payload, executable/
/// environment path, or login/session/cost information.
export interface ContextPackagePreparationDto {
  readonly workKind: WorkKind;
  readonly dataScope: ContextPackageDataScope;
  readonly consentedAtMs: number;
  readonly manifestCreatedAtMs: number;
}

const CONTEXT_PACKAGE_WORK_KINDS: readonly WorkKind[] = ["planning", "implementation", "review"];
const CONTEXT_PACKAGE_DATA_SCOPES: readonly ContextPackageDataScope[] = ["contextPackageV1"];
const CONTEXT_PACKAGE_PREPARATION_KEYS = [
  "workKind",
  "dataScope",
  "consentedAtMs",
  "manifestCreatedAtMs",
] as const;

/// Fail-closed runtime guard: rejects an unknown/malformed response, an
/// unrecognized `workKind`, any `dataScope` other than `"contextPackageV1"`,
/// and a negative or non-finite timestamp (`NaN`/`Infinity`), instead of
/// accepting the response and silently coercing a bad value.
export function isContextPackagePreparationDto(
  value: unknown,
): value is ContextPackagePreparationDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === CONTEXT_PACKAGE_PREPARATION_KEYS.length &&
    keys.every((key) => CONTEXT_PACKAGE_PREPARATION_KEYS.some((allowed) => allowed === key)) &&
    typeof value.workKind === "string" &&
    CONTEXT_PACKAGE_WORK_KINDS.includes(value.workKind as WorkKind) &&
    typeof value.dataScope === "string" &&
    CONTEXT_PACKAGE_DATA_SCOPES.includes(value.dataScope as ContextPackageDataScope) &&
    isNonNegativeFiniteNumber(value.consentedAtMs) &&
    isNonNegativeFiniteNumber(value.manifestCreatedAtMs)
  );
}

function isNonNegativeFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

/// Content-free read-only readiness signal: whether an exact
/// `(task_id, Claude, Planning, expectedVersion, ContextPackageV1)` consent
/// and its FK-bound manifest already exist. Carries nothing else -- no
/// consent/manifest value, timestamp, or task identity.
export interface ContextPackagePlanningReadinessDto {
  readonly ready: boolean;
}

const CONTEXT_PACKAGE_PLANNING_READINESS_KEYS = ["ready"] as const;

/// Fail-closed runtime guard: rejects anything other than an object with
/// exactly one boolean `ready` field, instead of coercing a truthy/falsy
/// value or defaulting a malformed response to `false`.
export function isContextPackagePlanningReadinessDto(
  value: unknown,
): value is ContextPackagePlanningReadinessDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === CONTEXT_PACKAGE_PLANNING_READINESS_KEYS.length &&
    keys.every((key) => CONTEXT_PACKAGE_PLANNING_READINESS_KEYS.some((allowed) => allowed === key)) &&
    typeof value.ready === "boolean"
  );
}

/// Content-free read-only readiness signal: whether an exact
/// `(task_id, Claude, Implementation, expectedVersion, ContextPackageV1)`
/// consent and its FK-bound manifest already exist. Carries nothing else --
/// no consent/manifest value, timestamp, or task identity. Says nothing
/// about whether a completed stored Claude Planning result exists -- that
/// is a separate structural precondition checked only when actually
/// starting Implementation.
export interface ContextPackageImplementationReadinessDto {
  readonly ready: boolean;
}

const CONTEXT_PACKAGE_IMPLEMENTATION_READINESS_KEYS = ["ready"] as const;

/// Fail-closed runtime guard: rejects anything other than an object with
/// exactly one boolean `ready` field, instead of coercing a truthy/falsy
/// value or defaulting a malformed response to `false`.
export function isContextPackageImplementationReadinessDto(
  value: unknown,
): value is ContextPackageImplementationReadinessDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === CONTEXT_PACKAGE_IMPLEMENTATION_READINESS_KEYS.length &&
    keys.every((key) =>
      CONTEXT_PACKAGE_IMPLEMENTATION_READINESS_KEYS.some((allowed) => allowed === key),
    ) &&
    typeof value.ready === "boolean"
  );
}

/// Content-free read-only readiness signal: whether an exact
/// `(task_id, Claude, Review, expectedVersion, ContextPackageV1)` consent
/// and its FK-bound manifest already exist. Carries nothing else -- no
/// consent/manifest value, timestamp, task identity, or diff content.
export interface ContextPackageReviewReadinessDto {
  readonly ready: boolean;
}

const CONTEXT_PACKAGE_REVIEW_READINESS_KEYS = ["ready"] as const;

/// Fail-closed runtime guard: rejects anything other than an object with
/// exactly one boolean `ready` field, instead of coercing a truthy/falsy
/// value or defaulting a malformed response to `false`.
export function isContextPackageReviewReadinessDto(
  value: unknown,
): value is ContextPackageReviewReadinessDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === CONTEXT_PACKAGE_REVIEW_READINESS_KEYS.length &&
    keys.every((key) => CONTEXT_PACKAGE_REVIEW_READINESS_KEYS.some((allowed) => allowed === key)) &&
    typeof value.ready === "boolean"
  );
}


export interface VersionDto {
  version: string;
}

export interface HealthDto {
  status: HealthState;
}

export interface ActiveTaskStatusDto {
  status: ActiveTaskStatusKind;
  taskId: string | null;
  acquiredAtMs: number | null;
}

export interface BootstrapStatusDto {
  storageStatus: StorageStatus;
  databaseStatus: DatabaseStatus;
  loggingStatus: LoggingStatus;
  activeTaskStatus: ActiveTaskStatusDto;
  applicationVersion: string;
  ready: boolean;
}

export interface LegacyMigrationDiagnosticDto {
  projectId: string;
  displayPath: string;
  reasonCode: string;
}

export interface CapabilityDto {
  secureStorage: CapabilityStatus;
  nativePermissions: CapabilityStatus;
  gitExecution: CapabilityStatus;
  claudeExecution: CapabilityStatus;
  codexExecution: CapabilityStatus;
  updater: CapabilityStatus;
  installerManagement: CapabilityStatus;
}

export interface SystemStatusDto {
  applicationVersion: string;
  health: HealthState;
  storageStatus: StorageStatus;
  databaseStatus: DatabaseStatus;
  loggingStatus: LoggingStatus;
  activeTaskStatus: ActiveTaskStatusDto;
  capabilities: CapabilityDto;
}

export interface ProjectDto {
  id: string;
  name: string;
  displayPath: string;
  createdAtMs: number;
  updatedAtMs: number;
}

export type RepositoryKind = "git" | "nonGit";
export interface RepositoryStatusDto { clean: boolean; detachedHead: boolean; currentBranch: string | null; headCommit: string | null; }
export interface ProjectCandidateDto { suggestedName: string; displayPath: string; confirmationToken: string; repositoryKind: RepositoryKind; repositoryStatus: RepositoryStatusDto | null; }
export interface ProjectStatusDto { projectId: string; repositoryKind: RepositoryKind; repositoryStatus: RepositoryStatusDto | null; }
export type GitIsolationStatus = "awaitingGitInitApproval" | "ready" | "gitInitInProgress" | "worktreeCreating" | "worktreeReady" | "recoveryRequired";
export type IsolationBlocker = "dirtyRepository" | "detachedHead" | "unbornRepository" | "missingCurrentBranch" | "gitAuthorMissing" | "gitOperationFailed" | "recoveryRequired";
export interface TaskIsolationDto { taskId: string; projectId: string; taskState: TaskState; taskVersion: number; isolationStatus: GitIsolationStatus; branchIdentity: string; baseBranch: string | null; baseCommit: string | null; blocker: IsolationBlocker | null; }

export interface ActiveTaskDto {
  taskId: string;
  acquiredAtMs: number;
}

export interface TaskBriefDto {
  requirements: string;
  completionCriteria: string;
  prohibitedScope: string;
  createdAtMs: number;
}

export interface TaskBriefInput {
  requirements: string;
  completionCriteria: string;
  prohibitedScope: string;
}

export interface TaskDto {
  id: string;
  projectId: string;
  state: TaskState;
  version: number;
  branchIdentity: string;
  resumeTargetState: TaskState | null;
  createdAtMs: number;
  updatedAtMs: number;
  terminalAtMs: number | null;
  brief: TaskBriefDto | null;
}

export interface CancelPlanningDto {
  requested: boolean;
}

export interface CancelImplementationDto {
  requested: boolean;
}

export interface CancelTestingDto {
  requested: boolean;
}

export interface CancelReviewDto {
  requested: boolean;
}

export interface MergeAbortStartDto {
  started: boolean;
}

export type ValidationCommandKind = "format" | "lint" | "typecheck" | "test" | "build";

export interface ValidationCommandCandidateDto {
  kind: ValidationCommandKind;
  label: string;
}

export interface ValidationCommandApprovalStatusDto {
  approvedKinds: readonly ValidationCommandKind[];
}

export interface ProjectRootValidationApprovalStatusDto {
  testApproved: boolean;
  buildApproved: boolean;
}

export interface ApproveValidationCommandInput {
  kinds: readonly ValidationCommandKind[];
  executablePath: string;
  cargoHomePath: string | null;
  rustupHomePath: string | null;
}

export interface ApproveValidationCommandResultDto {
  approvedKinds: readonly ValidationCommandKind[];
}

export interface ApproveProjectRootValidationInput {
  executablePath: string;
  cargoHomePath: string | null;
  rustupHomePath: string | null;
}

/// Exhaustive one-to-one mirror of the backend's 13 fixed
/// `HighRiskCategory` values. Fixed literal union only -- never a free-text
/// description, provider/work kind, or any diff/plan/provider-output/path/
/// auth/session/cost value.
export type HighRiskCategory =
  | "architectureChange"
  | "databaseSchemaChange"
  | "authenticationOrAuthorizationChange"
  | "securityPolicyChange"
  | "externalNetworkBehaviorAddition"
  | "externalDataTransmissionAddition"
  | "largeScaleFileMoveOrDeletion"
  | "publicApiOrStorageFormatChange"
  | "operatingSystemConfigurationChange"
  | "administratorPrivilegesRequired"
  | "breakingCompatibilityChange"
  | "dataMigration"
  | "difficultToRecoverChange";

/// Content-free: whether an exact `(taskId, expectedVersion, riskCategory)`
/// high-risk approval already exists. Carries nothing else.
export interface HighRiskApprovalStatusDto {
  approved: boolean;
}

/// Content-free approval result: which category was approved and when.
/// Never carries the task id, provider, work kind, or any source/diff/plan
/// content.
export interface HighRiskApprovalDto {
  riskCategory: HighRiskCategory;
  approvedAtMs: number;
}

/// The ONLY DTO in this codebase that carries raw repository diff content.
/// It exists solely for `getUserDiffForReview` to hand the diff, once,
/// directly to the requesting local user's own review modal -- never to a
/// provider, never persisted, never logged, never cached in a generic IPC
/// cache.
export interface RawUserDiffForReviewDto {
  diffText: string;
  diffContentHash: string;
}

/// Content-free approval result: only the timestamp the approval was
/// recorded at. Never carries the task id, the diff content hash, or any
/// diff text.
export interface UserDiffApprovalDto {
  approvedAtMs: number;
}

export type PlanningOutcome = "completed" | "failed" | "cancelled" | "recoveryRequired";

export interface PlanningResultDto {
  outcome: PlanningOutcome;
  exitCode: number | null;
  turnCount: number | null;
  startedAtMs: number;
  completedAtMs: number;
  planText: string | null;
}

export type PostMergeValidationCommandKind = "test" | "build";
export type PostMergeValidationOutcome =
  | "success"
  | "exitFailure"
  | "timedOut"
  | "stdoutBoundExceeded"
  | "bindingRejected"
  | "cancelled"
  | "uncertain";

export interface PostMergeValidationResultDto {
  commandKind: PostMergeValidationCommandKind;
  attemptSequence: number;
  outcome: PostMergeValidationOutcome;
  exitCode: number | null;
  safeSummary: string;
  startedAtMs: number;
  completedAtMs: number;
}

export type ReviewOutcome = "completed" | "failed" | "cancelled" | "recoveryRequired";

export interface ReviewResultDto {
  outcome: ReviewOutcome;
  exitCode: number | null;
  turnCount: number | null;
  startedAtMs: number;
  completedAtMs: number;
  reviewText: string | null;
}

export interface TaskTransitionDto {
  sequence: number;
  fromState: TaskState | null;
  toState: TaskState;
  taskVersion: number;
  occurredAtMs: number;
}

export interface SetClaudeExecutablePathDto {
  displayPath: string;
  claudeExecution: CapabilityStatus;
}

export type RefreshOutcome = "completed" | "superseded" | "conflict";

export interface RefreshClaudeCapabilityDto {
  outcome: RefreshOutcome;
  claudeExecution: CapabilityStatus;
  codexExecution: CapabilityStatus;
}

export interface IpcErrorDto {
  code: string;
  message: string;
  severity: IpcSeverity;
  retry: IpcRetry;
}
