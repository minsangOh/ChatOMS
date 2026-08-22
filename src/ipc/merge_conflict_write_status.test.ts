import "../test/setup";
import { describe, expect, it, vi } from "vitest";
import { createIpcClient, type InvokeTransport } from "./client";
import { FrontendError } from "./errors";

describe("merge conflict write status IPC", () => {
  it("calls the exact command name with only a task id and accepts a running response", async () => {
    const transport = vi.fn<InvokeTransport>(async () => ({ running: true }));
    const client = createIpcClient(transport);

    await expect(client.getMergeConflictWriteStatus("task-id")).resolves.toEqual({
      running: true,
    });
    expect(transport).toHaveBeenCalledWith("get_merge_conflict_write_status", {
      taskId: "task-id",
    });
  });

  it("accepts a well-formed idle response", async () => {
    const client = createIpcClient(async () => ({ running: false }));
    await expect(client.getMergeConflictWriteStatus("task-id")).resolves.toEqual({
      running: false,
    });
  });

  it("fail-closed rejects a non-boolean running field or a missing field", async () => {
    for (const malformed of [{ running: "true" }, { running: 1 }, { running: null }, {}]) {
      const client = createIpcClient(async () => malformed);
      await expect(client.getMergeConflictWriteStatus("task-id")).rejects.toMatchObject({
        code: "IPC_INVALID_RESPONSE",
      });
    }
  });

  // This response gates whether a Git-write action is offered, so a payload
  // that arrived carrying merge internals is a contract violation, not
  // something to read `running` out of and carry on from.
  it("fail-closed rejects a response carrying any merge detail alongside running", async () => {
    for (const leaked of [
      { running: true, path: "C:/projects/root" },
      { running: true, digest: "a".repeat(64) },
      { running: true, stdout: "CONFLICT (content): Merge conflict in tracked.txt" },
      { running: true, operation: "abort" },
      { running: false, taskId: "leaked-task-id" },
    ]) {
      const client = createIpcClient(async () => leaked);
      await expect(client.getMergeConflictWriteStatus("task-id")).rejects.toMatchObject({
        code: "IPC_INVALID_RESPONSE",
      });
    }
  });

  it("fail-closed rejects a non-object response", async () => {
    for (const malformed of [null, true, 1, "running", []]) {
      const client = createIpcClient(async () => malformed);
      await expect(client.getMergeConflictWriteStatus("task-id")).rejects.toMatchObject({
        code: "IPC_INVALID_RESPONSE",
      });
    }
  });

  it("preserves a safe error without exposing internal fields", async () => {
    const client = createIpcClient(async () => {
      throw {
        code: "APP_INVALID_INPUT",
        message: "The supplied data is invalid.",
        severity: "warning",
        retry: "never",
        source: "C:\\private\\database.sqlite",
      };
    });

    const error = await client
      .getMergeConflictWriteStatus("task-id")
      .catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(FrontendError);
    expect(error).toMatchObject({ code: "APP_INVALID_INPUT" });
    expect(JSON.stringify(error)).not.toContain("C:\\private");
  });
});
