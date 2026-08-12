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
  | "planningWithClaude"
  | "awaitingDesignApproval"
  | "implementingWithCodex"
  | "testing"
  | "autoFixing"
  | "reviewingWithClaude"
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
