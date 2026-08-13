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
    await client.getLegacyMigrationDiagnostic();
    await client.listProjects();
    await client.getActiveTask();
    await client.getTask("task-id");
    await client.listTaskHistory("task-id");

    expect(transport.mock.calls).toEqual([
      ["get_version", undefined],
      ["get_health", undefined],
      ["get_system_status", undefined],
      ["get_bootstrap_status", undefined],
      ["get_legacy_migration_diagnostic", undefined],
      ["list_projects", undefined],
      ["get_active_task", undefined],
      ["get_task", { taskId: "task-id" }],
      ["list_task_history", { taskId: "task-id" }],
    ]);
    expect(Object.values(IPC_COMMANDS)).toHaveLength(22);
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

  it("rejects a system status payload with an unrecognized provider capability value", async () => {
    const malformedCapabilities = {
      ...systemStatus,
      capabilities: { ...systemStatus.capabilities, claudeExecution: "unknown" },
    };
    const client = createIpcClient(async () => malformedCapabilities);
    await expect(client.getSystemStatus()).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
      message: "The application returned an invalid response.",
    });
  });

  it("uses purpose-specific Phase 2 mutation commands with version-bound payloads", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.inspectProjectCandidate("C:\\repo");
    await client.registerProject("C:\\repo", "token");
    await client.getProjectGitStatus("project-id");
    await client.createIsolationTask("project-id", { requirements: "req", completionCriteria: "crit", prohibitedScope: "scope" });
    await client.getTaskIsolation("task-id");
    await client.approveGitInitialization("task-id", 1);
    await client.createTaskWorktree("task-id", 2);
    expect(transport.mock.calls).toEqual([
      ["inspect_project_candidate", { inputPath: "C:\\repo" }],
      ["register_project", { inputPath: "C:\\repo", confirmationToken: "token", name: null }],
      ["get_project_git_status", { projectId: "project-id" }],
      ["create_isolation_task", { projectId: "project-id", brief: { requirements: "req", completionCriteria: "crit", prohibitedScope: "scope" } }],
      ["get_task_isolation", { taskId: "task-id" }],
      ["approve_git_initialization", { taskId: "task-id", expectedVersion: 1 }],
      ["create_task_worktree", { taskId: "task-id", expectedVersion: 2 }],
    ]);
  });

  it("uses provider-specific command names and payload shapes", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.setClaudeExecutablePath("C:\\claude.exe");
    await client.refreshClaudeCapability();
    expect(transport.mock.calls).toEqual([
      ["set_claude_executable_path", { path: "C:\\claude.exe" }],
      ["refresh_claude_capability", undefined],
    ]);
  });

  it("uses versioned task-scoped payloads for Claude Planning start and cancel", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.startClaudePlanning("task-id", 1);
    await client.cancelClaudePlanning("task-id");
    await client.getPlanningResult("task-id");
    expect(transport.mock.calls).toEqual([
      ["start_claude_planning", { taskId: "task-id", expectedVersion: 1 }],
      ["cancel_claude_planning", { taskId: "task-id" }],
      ["get_planning_result", { taskId: "task-id" }],
    ]);
  });

  it("validates Claude Planning response shapes and rejects malformed data", async () => {
    const valid = createIpcClient(async () => responses.start_claude_planning);
    await expect(valid.startClaudePlanning("task-id", 1)).resolves.toMatchObject({
      state: "planning",
    });

    const malformedCancel = createIpcClient(async () => ({ requested: "yes" }));
    await expect(malformedCancel.cancelClaudePlanning("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("returns a read-only planning result only for a recognized safe shape, and null for none", async () => {
    const valid = createIpcClient(async () => responses.get_planning_result);
    await expect(valid.getPlanningResult("task-id")).resolves.toMatchObject({
      outcome: "completed",
      planText: "Add a CSV export button.",
    });

    const none = createIpcClient(async () => null);
    await expect(none.getPlanningResult("task-id")).resolves.toBeNull();

    const planningResultFixture = responses.get_planning_result as Record<string, unknown>;
    const unknownOutcome = createIpcClient(async () => ({
      ...planningResultFixture,
      outcome: "unknownOutcome",
    }));
    await expect(unknownOutcome.getPlanningResult("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const extraField = createIpcClient(async () => ({
      ...planningResultFixture,
      sessionId: "should-never-appear",
    }));
    await expect(extraField.getPlanningResult("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const malformedShape = createIpcClient(async () => ({ outcome: "completed" }));
    await expect(malformedShape.getPlanningResult("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("validates provider response shapes and rejects malformed data", async () => {
    const valid = createIpcClient(async () => responses.set_claude_executable_path);
    await expect(valid.setClaudeExecutablePath("path")).resolves.toMatchObject({
      displayPath: "%USERPROFILE%\\claude.exe",
      claudeExecution: "unavailable",
    });

    const malformedRefresh = createIpcClient(async () => ({
      outcome: "invalid",
      claudeExecution: "supported",
      codexExecution: "unsupported",
    }));
    await expect(malformedRefresh.refreshClaudeCapability()).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("accepts provider-neutral task states and rejects legacy provider-bound values", async () => {
    const taskResponse = (state: string) => ({
      id: "task-id",
      projectId: "project-id",
      state,
      version: 3,
      branchIdentity: "ai-task/task-id",
      resumeTargetState: null,
      createdAtMs: 1,
      updatedAtMs: 2,
      terminalAtMs: null,
      brief: null,
    });

    for (const state of ["planning", "implementing", "reviewing"]) {
      const client = createIpcClient(async () => taskResponse(state));
      await expect(client.getTask("task-id")).resolves.toMatchObject({ state });
    }
    for (const state of [
      "planningWithClaude",
      "implementingWithCodex",
      "reviewingWithClaude",
    ]) {
      const client = createIpcClient(async () => taskResponse(state));
      await expect(client.getTask("task-id")).rejects.toMatchObject({
        code: "IPC_INVALID_RESPONSE",
      });
    }
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
  get_legacy_migration_diagnostic: null,
  list_projects: [],
  inspect_project_candidate: { suggestedName: "Repo", displayPath: "%USERPROFILE%\\repo", confirmationToken: "token", repositoryKind: "git", repositoryStatus: { clean: true, detachedHead: false, currentBranch: "main", headCommit: "a".repeat(40) } },
  register_project: { id: "project-id", name: "Repo", displayPath: "%USERPROFILE%\\repo", createdAtMs: 1, updatedAtMs: 1 },
  get_project_git_status: { projectId: "project-id", repositoryKind: "git", repositoryStatus: { clean: true, detachedHead: false, currentBranch: "main", headCommit: "a".repeat(40) } },
  create_isolation_task: { taskId: "task-id", projectId: "project-id", taskState: "projectValidated", taskVersion: 1, isolationStatus: "ready", branchIdentity: "ai-task/task-id", baseBranch: null, baseCommit: null, blocker: null },
  get_task_isolation: { taskId: "task-id", projectId: "project-id", taskState: "projectValidated", taskVersion: 1, isolationStatus: "ready", branchIdentity: "ai-task/task-id", baseBranch: null, baseCommit: null, blocker: null },
  approve_git_initialization: { taskId: "task-id", projectId: "project-id", taskState: "gitInitialized", taskVersion: 2, isolationStatus: "ready", branchIdentity: "ai-task/task-id", baseBranch: null, baseCommit: null, blocker: null },
  create_task_worktree: { taskId: "task-id", projectId: "project-id", taskState: "worktreeReady", taskVersion: 3, isolationStatus: "worktreeReady", branchIdentity: "ai-task/task-id", baseBranch: "main", baseCommit: "a".repeat(40), blocker: null },
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
    brief: null,
  },
  list_task_history: [],
  set_claude_executable_path: { displayPath: "%USERPROFILE%\\claude.exe", claudeExecution: "unavailable" },
  refresh_claude_capability: { outcome: "completed", claudeExecution: "supported", codexExecution: "unsupported" },
  start_claude_planning: {
    id: "task-id",
    projectId: "project-id",
    state: "planning",
    version: 2,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    terminalAtMs: null,
    brief: null,
  },
  cancel_claude_planning: { requested: true },
  get_planning_result: {
    outcome: "completed",
    exitCode: 0,
    turnCount: 3,
    startedAtMs: 1,
    completedAtMs: 2,
    planText: "Add a CSV export button.",
  },
};
