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

export type ValidationCommandKind = "format" | "lint" | "typecheck" | "test" | "build";

export interface ValidationCommandCandidateDto {
  kind: ValidationCommandKind;
  label: string;
}

export interface ValidationCommandApprovalStatusDto {
  approvedKinds: readonly ValidationCommandKind[];
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

export type PlanningOutcome = "completed" | "failed" | "cancelled" | "recoveryRequired";

export interface PlanningResultDto {
  outcome: PlanningOutcome;
  exitCode: number | null;
  turnCount: number | null;
  startedAtMs: number;
  completedAtMs: number;
  planText: string | null;
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
