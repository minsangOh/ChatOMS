import "../test/setup";
import { describe, expect, it, vi } from "vitest";
import { createIpcClient, type InvokeTransport } from "./client";
import { FrontendError } from "./errors";

describe("context package planning readiness IPC", () => {
  it("calls the exact command name and accepts a well-formed ready response", async () => {
    const transport = vi.fn<InvokeTransport>(async () => ({ ready: true }));
    const client = createIpcClient(transport);

    await expect(
      client.getContextPackagePlanningReadiness("task-id", 6),
    ).resolves.toEqual({ ready: true });
    expect(transport).toHaveBeenCalledWith("get_context_package_planning_readiness", {
      taskId: "task-id",
      expectedVersion: 6,
    });
  });

  it("accepts a well-formed not-ready response", async () => {
    const client = createIpcClient(async () => ({ ready: false }));
    await expect(
      client.getContextPackagePlanningReadiness("task-id", 6),
    ).resolves.toEqual({ ready: false });
  });

  it("fail-closed rejects a non-boolean ready field", async () => {
    for (const malformed of [{ ready: "true" }, { ready: 1 }, { ready: null }]) {
      const client = createIpcClient(async () => malformed);
      await expect(
        client.getContextPackagePlanningReadiness("task-id", 6),
      ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
    }
  });

  it("fail-closed rejects an unexpected extra field or a missing field", async () => {
    for (const malformed of [{ ready: true, taskId: "leaked-task-id" }, {}]) {
      const client = createIpcClient(async () => malformed);
      await expect(
        client.getContextPackagePlanningReadiness("task-id", 6),
      ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
    }
  });

  it("preserves a safe precondition error without exposing internal fields", async () => {
    const client = createIpcClient(async () => {
      throw {
        code: "APP_INVALID_STATE",
        message: "This task is not in a state that allows this action.",
        severity: "error",
        retry: "afterStateRefresh",
        source: "C:\\private\\database.sqlite",
      };
    });

    const error = await client
      .getContextPackagePlanningReadiness("task-id", 1)
      .catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(FrontendError);
    expect(error).toMatchObject({ code: "APP_INVALID_STATE" });
    expect(JSON.stringify(error)).not.toContain("C:\\private");
  });
});
