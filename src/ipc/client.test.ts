import "../test/setup";
import { describe, expect, it, vi } from "vitest";
import { createIpcClient, IPC_COMMANDS, type InvokeTransport } from "./client";
import { FrontendError } from "./errors";
import { bootstrapStatus, health, systemStatus, version } from "../test/fixtures";

describe("typed IPC client", () => {
  it("uses the approved command names and camelCase task payloads", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);

    await client.getVersion();
    await client.getHealth();
    await client.getSystemStatus();
    await client.getBootstrapStatus();
    await client.listProjects();
    await client.getActiveTask();
    await client.getTask("task-id");
    await client.listTaskHistory("task-id");

    expect(transport.mock.calls).toEqual([
      ["get_version", undefined],
      ["get_health", undefined],
      ["get_system_status", undefined],
      ["get_bootstrap_status", undefined],
      ["list_projects", undefined],
      ["get_active_task", undefined],
      ["get_task", { taskId: "task-id" }],
      ["list_task_history", { taskId: "task-id" }],
    ]);
    expect(Object.values(IPC_COMMANDS)).toHaveLength(8);
  });

  it("returns a validated result and rejects malformed success data safely", async () => {
    const success = createIpcClient(async () => version);
    await expect(success.getVersion()).resolves.toEqual(version);

    const malformed = createIpcClient(async () => ({ version: 1, source: "C:\\private" }));
    await expect(malformed.getVersion()).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
      message: "The application returned an invalid response.",
    });
  });

  it("keeps only approved IPC error fields and masks string or unknown failures", async () => {
    const ipcError = createIpcClient(async () => {
      throw {
        code: "APP_STORAGE_UNAVAILABLE",
        message: "Secure local storage is unavailable.",
        severity: "error",
        retry: "afterUserAction",
        source: "SELECT token FROM C:\\private S-1-5-21",
      };
    });
    const approved = await ipcError.getHealth().catch((error: unknown) => error);
    expect(approved).toBeInstanceOf(FrontendError);
    expect(approved).toMatchObject({ code: "APP_STORAGE_UNAVAILABLE" });
    expect(JSON.stringify(approved)).not.toMatch(/SELECT|C:\\private|S-1-5-21|token/);

    const stringFailure = createIpcClient(async () => {
      throw "C:\\private\\database.sqlite SELECT secret";
    });
    await expect(stringFailure.getHealth()).rejects.toMatchObject({
      code: "IPC_REQUEST_FAILED",
      message: "The request could not be completed.",
    });

    const unknownFailure = createIpcClient(async () => {
      throw { stack: "private stack", token: "secret" };
    });
    await expect(unknownFailure.getHealth()).rejects.toMatchObject({
      code: "APP_UNEXPECTED",
      message: "An unexpected error occurred.",
      retry: "never",
    });
  });
});

const responses: Record<string, unknown> = {
  get_version: version,
  get_health: health,
  get_system_status: systemStatus,
  get_bootstrap_status: bootstrapStatus,
  list_projects: [],
  get_active_task: null,
  get_task: {
    id: "task-id",
    projectId: "project-id",
    state: "created",
    version: 0,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 1,
    terminalAtMs: null,
  },
  list_task_history: [],
};
