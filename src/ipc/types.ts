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
  createdAtMs: number;
  updatedAtMs: number;
}

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

export interface IpcErrorDto {
  code: string;
  message: string;
  severity: IpcSeverity;
  retry: IpcRetry;
}
