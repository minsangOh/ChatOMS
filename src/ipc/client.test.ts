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
    expect(Object.values(IPC_COMMANDS)).toHaveLength(55);
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

  it("uses the read-only post-merge validation result command", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.getPostMergeValidationResults("task-id");
    expect(transport.mock.calls).toEqual([
      ["get_post_merge_validation_results", { taskId: "task-id" }],
    ]);
  });

  it("uses the read-only merge-conflict inspection command and rejects content-bearing fields", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await expect(client.getMergeConflictInspection("task-id")).resolves.toEqual(
      responses.get_merge_conflict_inspection,
    );
    expect(transport.mock.calls).toEqual([
      ["get_merge_conflict_inspection", { taskId: "task-id" }],
    ]);

    const unsafe = createIpcClient(async () => ({
      ...(responses.get_merge_conflict_inspection as Record<string, unknown>),
      path: "tracked.txt",
    }));
    await expect(unsafe.getMergeConflictInspection("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const none = createIpcClient(async () => null);
    await expect(none.getMergeConflictInspection("task-id")).resolves.toBeNull();
  });

  it("rejects post-merge results that contain unsafe or partial fields", async () => {
    const valid = createIpcClient(async () => responses.get_post_merge_validation_results);
    await expect(valid.getPostMergeValidationResults("task-id")).resolves.toHaveLength(2);

    const malformed = createIpcClient(async () => [{
      commandKind: "test",
      attemptSequence: 1,
      outcome: "success",
      exitCode: 0,
      safeSummary: "ok",
      startedAtMs: 1,
      completedAtMs: 2,
      projectRootPath: "C:\\\\private",
    }]);
    await expect(malformed.getPostMergeValidationResults("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("uses versioned task-scoped payloads for Claude Implementation start and cancel", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.startClaudeImplementation("task-id", 4);
    await client.cancelClaudeImplementation("task-id");
    expect(transport.mock.calls).toEqual([
      ["start_claude_implementation", { taskId: "task-id", expectedVersion: 4 }],
      ["cancel_claude_implementation", { taskId: "task-id" }],
    ]);
  });

  it("validates Claude Implementation response shapes and rejects malformed data", async () => {
    const valid = createIpcClient(async () => responses.start_claude_implementation);
    await expect(valid.startClaudeImplementation("task-id", 4)).resolves.toMatchObject({
      state: "implementing",
    });

    const malformedCancel = createIpcClient(async () => ({ requested: "yes" }));
    await expect(malformedCancel.cancelClaudeImplementation("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
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

  it("uses versioned task-scoped payloads for Cargo-only Testing start, cancel, and validation command IPC", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.startValidationTesting("task-id", 6);
    await client.cancelValidationTesting("task-id");
    await client.getValidationCommandCandidates("task-id");
    await client.getValidationCommandApprovalStatus("task-id");
    await client.approveValidationCommand("task-id", 6, {
      kinds: ["test"],
      executablePath: "C:\\tools\\cargo\\bin\\cargo.exe",
      cargoHomePath: null,
      rustupHomePath: null,
    });
    await client.getProjectRootValidationApprovalStatus("task-id", 6);
    await client.approveProjectRootValidation("task-id", 6, {
      executablePath: "C:\\tools\\cargo\\bin\\cargo.exe",
      cargoHomePath: null,
      rustupHomePath: null,
    });
    expect(transport.mock.calls).toEqual([
      ["start_validation_testing", { taskId: "task-id", expectedVersion: 6 }],
      ["cancel_validation_testing", { taskId: "task-id" }],
      ["get_validation_command_candidates", { taskId: "task-id" }],
      ["get_validation_command_approval_status", { taskId: "task-id" }],
      [
        "approve_validation_command",
        {
          taskId: "task-id",
          expectedVersion: 6,
          input: {
            kinds: ["test"],
            executablePath: "C:\\tools\\cargo\\bin\\cargo.exe",
            cargoHomePath: null,
            rustupHomePath: null,
          },
        },
      ],
      ["get_project_root_validation_approval_status", { taskId: "task-id", expectedVersion: 6 }],
      [
        "approve_project_root_validation",
        {
          taskId: "task-id",
          expectedVersion: 6,
          input: {
            executablePath: "C:\\tools\\cargo\\bin\\cargo.exe",
            cargoHomePath: null,
            rustupHomePath: null,
          },
        },
      ],
    ]);
  });

  it("uses versioned task-scoped payloads for Claude Review start, cancel, and result", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.startClaudeReview("task-id", 7);
    await client.cancelClaudeReview("task-id");
    await client.getReviewResult("task-id");
    expect(transport.mock.calls).toEqual([
      ["start_claude_review", { taskId: "task-id", expectedVersion: 7 }],
      ["cancel_claude_review", { taskId: "task-id" }],
      ["get_review_result", { taskId: "task-id" }],
    ]);
  });

  it("validates Claude Review response shapes and rejects malformed data", async () => {
    const valid = createIpcClient(async () => responses.start_claude_review);
    await expect(valid.startClaudeReview("task-id", 7)).resolves.toMatchObject({
      state: "reviewing",
    });

    const malformedCancel = createIpcClient(async () => ({ requested: "yes" }));
    await expect(malformedCancel.cancelClaudeReview("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("returns a read-only review result only for a recognized safe shape, and null for none", async () => {
    const valid = createIpcClient(async () => responses.get_review_result);
    await expect(valid.getReviewResult("task-id")).resolves.toMatchObject({
      outcome: "completed",
      reviewText: "The change matches the requirements.",
    });

    const none = createIpcClient(async () => null);
    await expect(none.getReviewResult("task-id")).resolves.toBeNull();

    const reviewResultFixture = responses.get_review_result as Record<string, unknown>;
    const unknownOutcome = createIpcClient(async () => ({
      ...reviewResultFixture,
      outcome: "unknownOutcome",
    }));
    await expect(unknownOutcome.getReviewResult("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const extraField = createIpcClient(async () => ({
      ...reviewResultFixture,
      sessionId: "should-never-appear",
    }));
    await expect(extraField.getReviewResult("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const malformedShape = createIpcClient(async () => ({ outcome: "completed" }));
    await expect(malformedShape.getReviewResult("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("validates Cargo-only Testing response shapes and rejects malformed data", async () => {
    const valid = createIpcClient(async () => responses.start_validation_testing);
    await expect(valid.startValidationTesting("task-id", 6)).resolves.toMatchObject({ state: "testing" });

    const malformedCancel = createIpcClient(async () => ({ requested: "yes" }));
    await expect(malformedCancel.cancelValidationTesting("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const malformedKindCandidate = createIpcClient(async () => [{ kind: "unknownKind", label: "x" }]);
    await expect(malformedKindCandidate.getValidationCommandCandidates("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const extraFieldCandidate = createIpcClient(async () => [
      { kind: "test", label: "Test", executable: "cargo" },
    ]);
    await expect(extraFieldCandidate.getValidationCommandCandidates("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const malformedStatus = createIpcClient(async () => ({ approvedKinds: ["notAKind"] }));
    await expect(malformedStatus.getValidationCommandApprovalStatus("task-id")).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const extraFieldApprove = createIpcClient(async () => ({ approvedKinds: ["test"], executablePath: "leaked" }));
    await expect(
      extraFieldApprove.approveValidationCommand("task-id", 6, {
        kinds: ["test"],
        executablePath: "x",
        cargoHomePath: null,
        rustupHomePath: null,
      }),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const malformedProjectRootStatus = createIpcClient(async () => ({ testApproved: true, buildApproved: "yes" }));
    await expect(
      malformedProjectRootStatus.getProjectRootValidationApprovalStatus("task-id", 6),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const leakedProjectRootStatus = createIpcClient(async () => ({ testApproved: true, buildApproved: true, executablePath: "leaked" }));
    await expect(
      leakedProjectRootStatus.approveProjectRootValidation("task-id", 6, {
        executablePath: "x",
        cargoHomePath: null,
        rustupHomePath: null,
      }),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
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

  it("uses versioned task-scoped payloads for high-risk approval status and approve", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.getHighRiskApprovalStatus("task-id", 3, "dataMigration");
    await client.approveHighRiskOperation("task-id", 3, "dataMigration");
    expect(transport.mock.calls).toEqual([
      [
        "get_high_risk_approval_status",
        { taskId: "task-id", expectedVersion: 3, riskCategory: "dataMigration" },
      ],
      [
        "approve_high_risk_operation",
        { taskId: "task-id", expectedVersion: 3, riskCategory: "dataMigration" },
      ],
    ]);
  });

  it("validates high-risk approval response shapes and rejects malformed data", async () => {
    const valid = createIpcClient(async () => responses.get_high_risk_approval_status);
    await expect(valid.getHighRiskApprovalStatus("task-id", 3, "dataMigration")).resolves.toEqual({
      approved: false,
    });

    const validApproval = createIpcClient(async () => responses.approve_high_risk_operation);
    await expect(
      validApproval.approveHighRiskOperation("task-id", 3, "dataMigration"),
    ).resolves.toEqual({ riskCategory: "dataMigration", approvedAtMs: 100 });

    const malformedStatus = createIpcClient(async () => ({ approved: "yes" }));
    await expect(
      malformedStatus.getHighRiskApprovalStatus("task-id", 3, "dataMigration"),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const extraFieldStatus = createIpcClient(async () => ({ approved: true, extra: "leaked" }));
    await expect(
      extraFieldStatus.getHighRiskApprovalStatus("task-id", 3, "dataMigration"),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const unknownCategory = createIpcClient(async () => ({
      riskCategory: "notACategory",
      approvedAtMs: 1,
    }));
    await expect(
      unknownCategory.approveHighRiskOperation("task-id", 3, "dataMigration"),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const malformedTimestamp = createIpcClient(async () => ({
      riskCategory: "dataMigration",
      approvedAtMs: "not-a-number",
    }));
    await expect(
      malformedTimestamp.approveHighRiskOperation("task-id", 3, "dataMigration"),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const extraFieldApproval = createIpcClient(async () => ({
      riskCategory: "dataMigration",
      approvedAtMs: 1,
      path: "C:\\leaked",
    }));
    await expect(
      extraFieldApproval.approveHighRiskOperation("task-id", 3, "dataMigration"),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
  });

  it("uses exact versioned payloads for Provider Implementation risk assessment", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.getProviderImplementationRiskAssessmentStatus("task-id", 3);
    await client.declareProviderImplementationRisk("task-id", 3, ["dataMigration"], false);
    await client.declareProviderImplementationRisk("task-id", 3, [], true);
    expect(transport.mock.calls).toEqual([
      [
        "get_provider_implementation_risk_assessment_status",
        { taskId: "task-id", expectedVersion: 3 },
      ],
      [
        "declare_provider_implementation_risk",
        {
          taskId: "task-id",
          expectedVersion: 3,
          riskCategories: ["dataMigration"],
          explicitEmpty: false,
        },
      ],
      [
        "declare_provider_implementation_risk",
        { taskId: "task-id", expectedVersion: 3, riskCategories: [], explicitEmpty: true },
      ],
    ]);
  });

  it("rejects expanded Provider Implementation risk assessment responses", async () => {
    for (const field of ["path", "digest", "stdout", "operation"]) {
      const client = createIpcClient(async () => ({
        assessmentRequired: true,
        declarationExists: false,
        selectedCategories: [],
        approvalReadiness: riskAssessmentReadiness,
        failureCategory: null,
        [field]: "leaked",
      }));
      await expect(
        client.getProviderImplementationRiskAssessmentStatus("task-id", 3),
      ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
    }
  });

  it("uses versioned task-scoped payloads for user diff review approval and merge start", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await client.getUserDiffForReview("task-id", 3);
    await client.approveUserDiff("task-id", 3, "a".repeat(64));
    await client.approveUserDiffAndStartMerge("task-id", 3, "a".repeat(64));
    await client.confirmManualResolutionAndStartMergeContinue("task-id", 5);
    expect(transport.mock.calls).toEqual([
      ["get_user_diff_for_review", { taskId: "task-id", expectedVersion: 3 }],
      [
        "approve_user_diff",
        { taskId: "task-id", expectedVersion: 3, expectedDiffContentHash: "a".repeat(64) },
      ],
      [
        "approve_user_diff_and_start_merge",
        { taskId: "task-id", expectedVersion: 3, expectedDiffContentHash: "a".repeat(64) },
      ],
      [
        "confirm_manual_resolution_and_start_merge_continue",
        { taskId: "task-id", expectedVersion: 5 },
      ],
    ]);
  });

  it("validates user diff review/approval response shapes and rejects malformed data", async () => {
    const validDiff = createIpcClient(async () => responses.get_user_diff_for_review);
    await expect(validDiff.getUserDiffForReview("task-id", 3)).resolves.toEqual(
      responses.get_user_diff_for_review,
    );

    const validApproval = createIpcClient(async () => responses.approve_user_diff);
    await expect(
      validApproval.approveUserDiff("task-id", 3, "a".repeat(64)),
    ).resolves.toEqual({ approvedAtMs: 100 });

    const validMerge = createIpcClient(async () => responses.approve_user_diff_and_start_merge);
    await expect(
      validMerge.approveUserDiffAndStartMerge("task-id", 3, "a".repeat(64)),
    ).resolves.toEqual(responses.approve_user_diff_and_start_merge);

    const validMergeContinue = createIpcClient(
      async () => responses.confirm_manual_resolution_and_start_merge_continue,
    );
    await expect(
      validMergeContinue.confirmManualResolutionAndStartMergeContinue("task-id", 5),
    ).resolves.toEqual(responses.confirm_manual_resolution_and_start_merge_continue);

    // This command uses a dedicated exact-shape guard (`isExactTaskDto`), not
    // the general, deliberately loose `isTaskDto` every other command uses:
    // the response must carry exactly backend `TaskDto`'s serialized keys,
    // so a resolution digest, raw path, or Git stdout/stderr accidentally
    // attached to a future response is rejected rather than silently passed
    // through, and a response missing a required TaskDto field still fails
    // closed like any other task state read.
    const mergeContinueBase = responses
      .confirm_manual_resolution_and_start_merge_continue as Record<string, unknown>;

    const incompleteMergeContinue = createIpcClient(async () => {
      const { version: _version, ...rest } = mergeContinueBase;
      return rest;
    });
    await expect(
      incompleteMergeContinue.confirmManualResolutionAndStartMergeContinue("task-id", 5),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const wrongTypeMergeContinue = createIpcClient(async () => ({
      ...mergeContinueBase,
      version: "6",
    }));
    await expect(
      wrongTypeMergeContinue.confirmManualResolutionAndStartMergeContinue("task-id", 5),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const rawPathMergeContinue = createIpcClient(async () => ({
      ...mergeContinueBase,
      rootPath: "C:\\private\\project",
    }));
    await expect(
      rawPathMergeContinue.confirmManualResolutionAndStartMergeContinue("task-id", 5),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const digestMergeContinue = createIpcClient(async () => ({
      ...mergeContinueBase,
      resolutionDigest: "a".repeat(64),
    }));
    await expect(
      digestMergeContinue.confirmManualResolutionAndStartMergeContinue("task-id", 5),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const rawOutputMergeContinue = createIpcClient(async () => ({
      ...mergeContinueBase,
      stdout: "fatal: not a git repository",
    }));
    await expect(
      rawOutputMergeContinue.confirmManualResolutionAndStartMergeContinue("task-id", 5),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    // The general `isTaskDto` guard every other command relies on stays
    // deliberately loose: the same extra field that the exact guard above
    // rejects is still accepted here, confirming this new guard is scoped
    // to `confirmManualResolutionAndStartMergeContinue` only.
    const looseGuardStillAcceptsExtraFields = createIpcClient(async () => ({
      ...(responses.approve_user_diff_and_start_merge as Record<string, unknown>),
      resolutionDigest: "a".repeat(64),
    }));
    await expect(
      looseGuardStillAcceptsExtraFields.approveUserDiffAndStartMerge("task-id", 3, "a".repeat(64)),
    ).resolves.toMatchObject({ id: "task-id", state: "merging" });

    const malformedHash = createIpcClient(async () => ({
      diffText: "x",
      diffContentHash: "not-hex",
    }));
    await expect(malformedHash.getUserDiffForReview("task-id", 3)).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const nonStringDiff = createIpcClient(async () => ({
      diffText: 1,
      diffContentHash: "a".repeat(64),
    }));
    await expect(nonStringDiff.getUserDiffForReview("task-id", 3)).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const malformedTimestamp = createIpcClient(async () => ({ approvedAtMs: "not-a-number" }));
    await expect(
      malformedTimestamp.approveUserDiff("task-id", 3, "a".repeat(64)),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });

    const approvalWithRawDiff = createIpcClient(async () => ({
      approvedAtMs: 100,
      diffText: "leaked diff content",
    }));
    await expect(
      approvalWithRawDiff.approveUserDiff("task-id", 3, "a".repeat(64)),
    ).rejects.toMatchObject({ code: "IPC_INVALID_RESPONSE" });
  });

  it("uses a content-free command for confirming a merge abort and rejects extra fields", async () => {
    const transport = vi.fn<InvokeTransport>(async (command) => responses[command]);
    const client = createIpcClient(transport);
    await expect(client.confirmMergeAbortAndStart("task-id", 5)).resolves.toEqual({
      started: true,
    });
    expect(transport.mock.calls).toEqual([
      ["confirm_merge_abort_and_start", { taskId: "task-id", expectedVersion: 5 }],
    ]);

    const startedFalse = createIpcClient(async () => ({ started: false }));
    await expect(startedFalse.confirmMergeAbortAndStart("task-id", 5)).resolves.toEqual({
      started: false,
    });

    const wrongType = createIpcClient(async () => ({ started: "true" }));
    await expect(wrongType.confirmMergeAbortAndStart("task-id", 5)).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });

    const extraField = createIpcClient(async () => ({
      started: true,
      taskState: "cancelled",
    }));
    await expect(extraField.confirmMergeAbortAndStart("task-id", 5)).rejects.toMatchObject({
      code: "IPC_INVALID_RESPONSE",
    });
  });

  it("accepts the restoredPendingAbortConfirmation merge-conflict inspection outcome", async () => {
    const client = createIpcClient(async () => ({
      outcome: "restoredPendingAbortConfirmation",
      counts: {
        total: 0,
        bothModified: 0,
        bothAdded: 0,
        bothDeleted: 0,
        addedByUs: 0,
        addedByThem: 0,
        deletedByUs: 0,
        deletedByThem: 0,
      },
    }));
    await expect(client.getMergeConflictInspection("task-id")).resolves.toMatchObject({
      outcome: "restoredPendingAbortConfirmation",
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

const riskAssessmentReadiness = [
  "architectureChange",
  "databaseSchemaChange",
  "authenticationOrAuthorizationChange",
  "securityPolicyChange",
  "externalNetworkBehaviorAddition",
  "externalDataTransmissionAddition",
  "largeScaleFileMoveOrDeletion",
  "publicApiOrStorageFormatChange",
  "operatingSystemConfigurationChange",
  "administratorPrivilegesRequired",
  "breakingCompatibilityChange",
  "dataMigration",
  "difficultToRecoverChange",
].map((riskCategory) => ({ riskCategory, approved: riskCategory === "dataMigration" }));

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
  get_post_merge_validation_results: [
    {
      commandKind: "test",
      attemptSequence: 1,
      outcome: "success",
      exitCode: 0,
      safeSummary: "post-merge validation completed successfully",
      startedAtMs: 3,
      completedAtMs: 4,
    },
    {
      commandKind: "build",
      attemptSequence: 1,
      outcome: "success",
      exitCode: 0,
      safeSummary: "post-merge validation completed successfully",
      startedAtMs: 5,
      completedAtMs: 6,
    },
  ],
  get_merge_conflict_inspection: {
    outcome: "confirmedUnresolved",
    counts: {
      total: 2,
      bothModified: 1,
      bothAdded: 0,
      bothDeleted: 0,
      addedByUs: 0,
      addedByThem: 1,
      deletedByUs: 0,
      deletedByThem: 0,
    },
  },
  start_claude_implementation: {
    id: "task-id",
    projectId: "project-id",
    state: "implementing",
    version: 5,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    terminalAtMs: null,
    brief: null,
  },
  cancel_claude_implementation: { requested: true },
  start_validation_testing: {
    id: "task-id",
    projectId: "project-id",
    state: "testing",
    version: 6,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    terminalAtMs: null,
    brief: null,
  },
  cancel_validation_testing: { requested: true },
  get_validation_command_candidates: [{ kind: "test", label: "Test (cargo test)" }],
  get_validation_command_approval_status: { approvedKinds: ["test"] },
  approve_validation_command: { approvedKinds: ["test"] },
  get_project_root_validation_approval_status: { testApproved: true, buildApproved: true },
  approve_project_root_validation: { testApproved: true, buildApproved: true },
  start_claude_review: {
    id: "task-id",
    projectId: "project-id",
    state: "reviewing",
    version: 7,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    terminalAtMs: null,
    brief: null,
  },
  cancel_claude_review: { requested: true },
  get_review_result: {
    outcome: "completed",
    exitCode: 0,
    turnCount: 3,
    startedAtMs: 1,
    completedAtMs: 2,
    reviewText: "The change matches the requirements.",
  },
  get_high_risk_approval_status: { approved: false },
  approve_high_risk_operation: { riskCategory: "dataMigration", approvedAtMs: 100 },
  get_provider_implementation_risk_assessment_status: {
    assessmentRequired: true,
    declarationExists: false,
    selectedCategories: [],
    approvalReadiness: riskAssessmentReadiness,
    failureCategory: null,
  },
  declare_provider_implementation_risk: {
    assessmentRequired: false,
    declarationExists: true,
    selectedCategories: ["dataMigration"],
    approvalReadiness: riskAssessmentReadiness,
    failureCategory: null,
  },
  get_user_diff_for_review: {
    diffText: "diff --git a/x b/x\n+line\n",
    diffContentHash: "a".repeat(64),
  },
  approve_user_diff: { approvedAtMs: 100 },
  approve_user_diff_and_start_merge: {
    id: "task-id",
    projectId: "project-id",
    state: "merging",
    version: 4,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    terminalAtMs: null,
    brief: null,
  },
  confirm_manual_resolution_and_start_merge_continue: {
    id: "task-id",
    projectId: "project-id",
    state: "merging",
    version: 6,
    branchIdentity: "ai-task/task-id",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 2,
    terminalAtMs: null,
    brief: null,
  },
  confirm_merge_abort_and_start: { started: true },
};
