import type { IpcClient } from "../ipc/client";
import type {
  BootstrapStatusDto,
  HealthDto,
  ProjectDto,
  SystemStatusDto,
  VersionDto,
} from "../ipc/types";

export const version: VersionDto = { version: "0.1.0" };
export const health: HealthDto = { status: "healthy" };
export const bootstrapStatus: BootstrapStatusDto = {
  storageStatus: "ready",
  databaseStatus: "ready",
  loggingStatus: "ready",
  activeTaskStatus: { status: "none", taskId: null, acquiredAtMs: null },
  applicationVersion: "0.1.0",
  ready: true,
};
export const systemStatus: SystemStatusDto = {
  applicationVersion: "0.1.0",
  health: "healthy",
  storageStatus: "ready",
  databaseStatus: "ready",
  loggingStatus: "ready",
  activeTaskStatus: { status: "none", taskId: null, acquiredAtMs: null },
  capabilities: {
    secureStorage: "supported",
    nativePermissions: "supported",
    gitExecution: "supported",
    claudeExecution: "unavailable",
    codexExecution: "unavailable",
    updater: "unavailable",
    installerManagement: "unavailable",
  },
};

export function createFakeClient(overrides: Partial<IpcClient> = {}): IpcClient {
  return {
    getVersion: async () => version,
    getHealth: async () => health,
    getSystemStatus: async () => systemStatus,
    getBootstrapStatus: async () => bootstrapStatus,
    getLegacyMigrationDiagnostic: async () => null,
    listProjects: async (): Promise<ProjectDto[]> => [],
    inspectProjectCandidate: async () => { throw new Error("not implemented in frontend tests"); },
    registerProject: async () => { throw new Error("not implemented in frontend tests"); },
    getProjectGitStatus: async () => { throw new Error("not implemented in frontend tests"); },
    createIsolationTask: async () => { throw new Error("not implemented in frontend tests"); },
    getTaskIsolation: async () => { throw new Error("not implemented in frontend tests"); },
    approveGitInitialization: async () => { throw new Error("not implemented in frontend tests"); },
    createTaskWorktree: async () => { throw new Error("not implemented in frontend tests"); },
    getActiveTask: async () => null,
    getTask: async () => {
      throw new Error("not implemented in frontend tests");
    },
    listTaskHistory: async () => [],
    ...overrides,
  };
}
