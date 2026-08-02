import { invoke } from "@tauri-apps/api/core";
import { FrontendError, isRecord, toFrontendError } from "./errors";
import type {
  ActiveTaskDto,
  ActiveTaskStatusDto,
  BootstrapStatusDto,
  CapabilityDto,
  DatabaseStatus,
  HealthDto,
  HealthState,
  LoggingStatus,
  ProjectDto,
  StorageStatus,
  SystemStatusDto,
  TaskDto,
  TaskState,
  TaskTransitionDto,
  VersionDto,
} from "./types";

export const IPC_COMMANDS = {
  getVersion: "get_version",
  getHealth: "get_health",
  getSystemStatus: "get_system_status",
  getBootstrapStatus: "get_bootstrap_status",
  listProjects: "list_projects",
  getActiveTask: "get_active_task",
  getTask: "get_task",
  listTaskHistory: "list_task_history",
} as const;

export type InvokeTransport = (
  command: string,
  payload?: Record<string, unknown>,
) => Promise<unknown>;

export interface IpcClient {
  getVersion(): Promise<VersionDto>;
  getHealth(): Promise<HealthDto>;
  getSystemStatus(): Promise<SystemStatusDto>;
  getBootstrapStatus(): Promise<BootstrapStatusDto>;
  listProjects(): Promise<ProjectDto[]>;
  getActiveTask(): Promise<ActiveTaskDto | null>;
  getTask(taskId: string): Promise<TaskDto>;
  listTaskHistory(taskId: string): Promise<TaskTransitionDto[]>;
}

const tauriTransport: InvokeTransport = (command, payload) =>
  invoke<unknown>(command, payload);

export function createIpcClient(transport: InvokeTransport = tauriTransport): IpcClient {
  const request = async <T>(
    command: string,
    guard: (value: unknown) => value is T,
    payload?: Record<string, unknown>,
  ): Promise<T> => {
    try {
      const result = await transport(command, payload);
      if (!guard(result)) {
        throw new FrontendError({
          code: "IPC_INVALID_RESPONSE",
          message: "The application returned an invalid response.",
          severity: "error",
          retry: "never",
        });
      }
      return result;
    } catch (error: unknown) {
      throw toFrontendError(error);
    }
  };

  return {
    getVersion: () => request(IPC_COMMANDS.getVersion, isVersionDto),
    getHealth: () => request(IPC_COMMANDS.getHealth, isHealthDto),
    getSystemStatus: () => request(IPC_COMMANDS.getSystemStatus, isSystemStatusDto),
    getBootstrapStatus: () => request(IPC_COMMANDS.getBootstrapStatus, isBootstrapStatusDto),
    listProjects: () => request(IPC_COMMANDS.listProjects, isProjectDtoArray),
    getActiveTask: () => request(IPC_COMMANDS.getActiveTask, isNullableActiveTaskDto),
    getTask: (taskId) => request(IPC_COMMANDS.getTask, isTaskDto, { taskId }),
    listTaskHistory: (taskId) =>
      request(IPC_COMMANDS.listTaskHistory, isTaskTransitionDtoArray, { taskId }),
  };
}

export const ipcClient = createIpcClient();

const HEALTH_STATES: readonly HealthState[] = ["healthy", "degraded", "unavailable"];
const STORAGE_STATUSES: readonly StorageStatus[] = [
  "ready",
  "unavailable",
  "insecure",
  "unsupported",
];
const DATABASE_STATUSES: readonly DatabaseStatus[] = [
  "notChecked",
  "ready",
  "upgraded",
  "migrationRequired",
  "unavailable",
  "incompatible",
];
const LOGGING_STATUSES: readonly LoggingStatus[] = ["notChecked", "ready", "unavailable"];
const CAPABILITY_STATUSES = ["supported", "unsupported", "unavailable"] as const;
const ACTIVE_STATUSES = ["notChecked", "none", "active"] as const;
const TASK_STATES: readonly TaskState[] = [
  "created",
  "projectValidated",
  "awaitingGitInitApproval",
  "gitInitialized",
  "worktreeCreating",
  "worktreeReady",
  "planningWithClaude",
  "awaitingDesignApproval",
  "implementingWithCodex",
  "testing",
  "autoFixing",
  "reviewingWithClaude",
  "reviewFixing",
  "awaitingUserDiffApproval",
  "merging",
  "mergeConflict",
  "postMergeTesting",
  "completed",
  "paused",
  "failed",
  "recoveryRequired",
  "unknownExternalEffect",
  "cancelled",
  "cleanupPending",
  "archived",
];

function isVersionDto(value: unknown): value is VersionDto {
  return isRecord(value) && typeof value.version === "string";
}

function isHealthDto(value: unknown): value is HealthDto {
  return isRecord(value) && isOneOf(value.status, HEALTH_STATES);
}

function isActiveTaskStatusDto(value: unknown): value is ActiveTaskStatusDto {
  return (
    isRecord(value) &&
    isOneOf(value.status, ACTIVE_STATUSES) &&
    isNullableString(value.taskId) &&
    isNullableNumber(value.acquiredAtMs)
  );
}

function isBootstrapStatusDto(value: unknown): value is BootstrapStatusDto {
  return (
    isRecord(value) &&
    isOneOf(value.storageStatus, STORAGE_STATUSES) &&
    isOneOf(value.databaseStatus, DATABASE_STATUSES) &&
    isOneOf(value.loggingStatus, LOGGING_STATUSES) &&
    isActiveTaskStatusDto(value.activeTaskStatus) &&
    typeof value.applicationVersion === "string" &&
    typeof value.ready === "boolean"
  );
}

function isCapabilityDto(value: unknown): value is CapabilityDto {
  return (
    isRecord(value) &&
    isOneOf(value.secureStorage, CAPABILITY_STATUSES) &&
    isOneOf(value.nativePermissions, CAPABILITY_STATUSES) &&
    isOneOf(value.gitExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.claudeExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.codexExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.updater, CAPABILITY_STATUSES) &&
    isOneOf(value.installerManagement, CAPABILITY_STATUSES)
  );
}

function isSystemStatusDto(value: unknown): value is SystemStatusDto {
  return (
    isRecord(value) &&
    typeof value.applicationVersion === "string" &&
    isOneOf(value.health, HEALTH_STATES) &&
    isOneOf(value.storageStatus, STORAGE_STATUSES) &&
    isOneOf(value.databaseStatus, DATABASE_STATUSES) &&
    isOneOf(value.loggingStatus, LOGGING_STATUSES) &&
    isActiveTaskStatusDto(value.activeTaskStatus) &&
    isCapabilityDto(value.capabilities)
  );
}

function isProjectDto(value: unknown): value is ProjectDto {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.name === "string" &&
    typeof value.createdAtMs === "number" &&
    typeof value.updatedAtMs === "number"
  );
}

function isProjectDtoArray(value: unknown): value is ProjectDto[] {
  return Array.isArray(value) && value.every(isProjectDto);
}

function isActiveTaskDto(value: unknown): value is ActiveTaskDto {
  return isRecord(value) && typeof value.taskId === "string" && typeof value.acquiredAtMs === "number";
}

function isNullableActiveTaskDto(value: unknown): value is ActiveTaskDto | null {
  return value === null || isActiveTaskDto(value);
}

function isTaskDto(value: unknown): value is TaskDto {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.projectId === "string" &&
    isOneOf(value.state, TASK_STATES) &&
    typeof value.version === "number" &&
    typeof value.branchIdentity === "string" &&
    (value.resumeTargetState === null || isOneOf(value.resumeTargetState, TASK_STATES)) &&
    typeof value.createdAtMs === "number" &&
    typeof value.updatedAtMs === "number" &&
    isNullableNumber(value.terminalAtMs)
  );
}

function isTaskTransitionDto(value: unknown): value is TaskTransitionDto {
  return (
    isRecord(value) &&
    typeof value.sequence === "number" &&
    (value.fromState === null || isOneOf(value.fromState, TASK_STATES)) &&
    isOneOf(value.toState, TASK_STATES) &&
    typeof value.taskVersion === "number" &&
    typeof value.occurredAtMs === "number"
  );
}

function isTaskTransitionDtoArray(value: unknown): value is TaskTransitionDto[] {
  return Array.isArray(value) && value.every(isTaskTransitionDto);
}

function isOneOf<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && allowed.includes(value as T);
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isNullableNumber(value: unknown): value is number | null {
  return value === null || typeof value === "number";
}
