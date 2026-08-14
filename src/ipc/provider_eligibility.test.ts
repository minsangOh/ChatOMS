import "../test/setup";
import { describe, expect, it, vi } from "vitest";
import { createIpcClient, type InvokeTransport } from "./client";
import { FrontendError } from "./errors";
import type { ProviderEligibilityDto } from "./types";

const providerEligibility = [
  {
    workKind: "planning",
    provider: "claude",
    capability: "supported",
    contract: "approved",
    eligible: true,
    stateAllowsWorkKind: true,
    blockingReasons: [],
  },
  {
    workKind: "planning",
    provider: "codex",
    capability: "unsupported",
    contract: "notApproved",
    eligible: false,
    stateAllowsWorkKind: true,
    blockingReasons: ["capabilityUnsupported", "contractNotApproved"],
  },
  {
    workKind: "implementation",
    provider: "claude",
    capability: "supported",
    contract: "notApproved",
    eligible: false,
    stateAllowsWorkKind: false,
    blockingReasons: ["contractNotApproved", "taskStateMismatch"],
  },
  {
    workKind: "implementation",
    provider: "codex",
    capability: "unsupported",
    contract: "notApproved",
    eligible: false,
    stateAllowsWorkKind: false,
    blockingReasons: [
      "capabilityUnsupported",
      "contractNotApproved",
      "taskStateMismatch",
    ],
  },
  {
    workKind: "review",
    provider: "claude",
    capability: "supported",
    contract: "approved",
    eligible: true,
    stateAllowsWorkKind: false,
    blockingReasons: ["taskStateMismatch"],
  },
  {
    workKind: "review",
    provider: "codex",
    capability: "unsupported",
    contract: "notApproved",
    eligible: false,
    stateAllowsWorkKind: false,
    blockingReasons: [
      "capabilityUnsupported",
      "contractNotApproved",
      "taskStateMismatch",
    ],
  },
] satisfies readonly ProviderEligibilityDto[];

describe("provider eligibility IPC", () => {
  it("uses the read-only command and accepts the complete safe response", async () => {
    const transport = vi.fn<InvokeTransport>(async () => providerEligibility);
    const client = createIpcClient(transport);

    await expect(client.getProviderEligibility("task-id")).resolves.toEqual(
      providerEligibility,
    );
    expect(transport).toHaveBeenCalledWith("get_provider_eligibility", {
      taskId: "task-id",
    });
  });

  it("rejects malformed, incomplete, and extra-field eligibility responses", async () => {
    const invalidCapability = providerEligibility.map((entry, index) =>
      index === 0 ? { ...entry, capability: "unknown" } : entry,
    );
    const extraField = providerEligibility.map((entry, index) =>
      index === 0 ? { ...entry, rawPath: "private" } : entry,
    );
    const duplicateCombination = providerEligibility.map((entry, index) =>
      index === 1 ? { ...entry, provider: "claude" } : entry,
    );
    for (const malformed of [
      invalidCapability,
      extraField,
      duplicateCombination,
      providerEligibility.slice(0, 5),
    ]) {
      const client = createIpcClient(async () => malformed);
      await expect(client.getProviderEligibility("task-id")).rejects.toMatchObject({
        code: "IPC_INVALID_RESPONSE",
      });
    }
  });

  it("preserves the safe task-not-found error without exposing internal fields", async () => {
    const client = createIpcClient(async () => {
      throw {
        code: "APP_NOT_FOUND",
        message: "The requested item could not be found.",
        severity: "warning",
        retry: "afterStateRefresh",
        source: "C:\\private\\database.sqlite",
      };
    });

    const error = await client
      .getProviderEligibility("missing-task")
      .catch((failure: unknown) => failure);
    expect(error).toBeInstanceOf(FrontendError);
    expect(error).toMatchObject({ code: "APP_NOT_FOUND" });
    expect(JSON.stringify(error)).not.toContain("C:\\private");
  });
});
