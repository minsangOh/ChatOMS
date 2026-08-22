import "../test/setup";
import { describe, expect, it, vi } from "vitest";
import { createIpcClient, type InvokeTransport } from "./client";
import { FrontendError } from "./errors";
import type { ContextPackagePreparationDto } from "./types";

const preparation = {
  workKind: "review",
  dataScope: "contextPackageV1",
  consentedAtMs: 200,
  manifestCreatedAtMs: 210,
} satisfies ContextPackagePreparationDto;

describe("context package preparation IPC", () => {
  it("calls the exact command name and accepts a well-formed response", async () => {
    const transport = vi.fn<InvokeTransport>(async () => preparation);
    const client = createIpcClient(transport);

    await expect(
      client.prepareReviewContextPackage("task-id", 6),
    ).resolves.toEqual(preparation);
    expect(transport).toHaveBeenCalledWith("prepare_review_context_package", {
      taskId: "task-id",
      expectedVersion: 6,
    });
  });

  it("dispatches planning and implementation preparation to their own command names", async () => {
    const transport = vi.fn<InvokeTransport>(async () => preparation);
    const client = createIpcClient(transport);

    await client.preparePlanningContextPackage("task-id", 1);
    expect(transport).toHaveBeenCalledWith("prepare_planning_context_package", {
      taskId: "task-id",
      expectedVersion: 1,
    });

    await client.prepareImplementationContextPackage("task-id", 3);
    expect(transport).toHaveBeenCalledWith(
      "prepare_implementation_context_package",
      { taskId: "task-id", expectedVersion: 3 },
    );
  });

  it("fail-closed rejects a data scope other than contextPackageV1", async () => {
    const client = createIpcClient(
      async () => ({ ...preparation, dataScope: "legacyPhase4" }),
    );
    await expect(
      client.prepareReviewContextPackage("task-id", 6),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
  });

  it("fail-closed rejects an unrecognized work kind", async () => {
    const client = createIpcClient(
      async () => ({ ...preparation, workKind: "testing" }),
    );
    await expect(
      client.prepareReviewContextPackage("task-id", 6),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
  });

  it("fail-closed rejects a negative or non-finite timestamp", async () => {
    for (const malformed of [
      { ...preparation, consentedAtMs: -1 },
      { ...preparation, manifestCreatedAtMs: Number.NaN },
      { ...preparation, consentedAtMs: Number.POSITIVE_INFINITY },
    ]) {
      const client = createIpcClient(async () => malformed);
      await expect(
        client.prepareReviewContextPackage("task-id", 6),
      ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
    }
  });

  it("fail-closed rejects an unexpected extra field or a missing field", async () => {
    const withExtraField = { ...preparation, taskId: "leaked-task-id" };
    const { manifestCreatedAtMs: _omitted, ...withMissingField } = preparation;
    for (const malformed of [withExtraField, withMissingField]) {
      const client = createIpcClient(async () => malformed);
      await expect(
        client.prepareReviewContextPackage("task-id", 6),
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
      .preparePlanningContextPackage("task-id", 1)
      .catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(FrontendError);
    expect(error).toMatchObject({ code: "APP_INVALID_STATE" });
    expect(JSON.stringify(error)).not.toContain("C:\\private");
  });
});
