import "../test/setup";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { IpcClient } from "../ipc/client";
import { FrontendError } from "../ipc/errors";
import type { ProjectDto, ProviderEligibilityDto, TaskDto, TaskIsolationDto } from "../ipc/types";
import { createFakeClient } from "../test/fixtures";
import { ProjectsPage } from "./ProjectsPage";

const project: ProjectDto = {
  id: "01900000-0000-7000-8000-000000000001",
  name: "Foundation",
  displayPath: "%USERPROFILE%\\Foundation",
  createdAtMs: 1_700_000_000_000,
  updatedAtMs: 1_700_000_100_000,
};

function restoredIsolation(taskState: TaskIsolationDto["taskState"]): TaskIsolationDto {
  return {
    taskId: "task-active",
    projectId: project.id,
    taskState,
    taskVersion: 3,
    isolationStatus: taskState === "recoveryRequired" ? "recoveryRequired" : "ready",
    branchIdentity: "ai-task/task-active",
    baseBranch: null,
    baseCommit: null,
    blocker: taskState === "recoveryRequired" ? "recoveryRequired" : null,
  };
}

function restoredTask(state: TaskDto["state"]): TaskDto {
  return {
    id: "task-active",
    projectId: project.id,
    state,
    version: 3,
    branchIdentity: "ai-task/task-active",
    resumeTargetState: null,
    createdAtMs: 1,
    updatedAtMs: 1,
    terminalAtMs: null,
    brief: null,
  };
}

it("renders loading and then project registration alternatives", async () => {
  render(<ProjectsPage client={createFakeClient()} />);
  expect(screen.getByRole("status")).toHaveTextContent("Loading projects");
  expect(await screen.findByRole("heading", { name: "No projects" })).toBeVisible();
  expect(screen.getByText("Choose or enter a local directory to register a project.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Choose folder" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Inspect" })).toBeDisabled();
});

it("renders one or more projects with IDs and timestamps but never root paths", async () => {
  const forbiddenPathField = ["root", "Path"].join("");
  const projectWithForbiddenExtra = {
    ...project,
    [forbiddenPathField]: "C:\\private\\project",
  } as ProjectDto;
  render(
    <ProjectsPage
      client={createFakeClient({
        listProjects: async () => [projectWithForbiddenExtra, { ...project, id: "second", name: "Second" }],
      })}
    />,
  );
  expect(await screen.findByRole("heading", { name: "Foundation" })).toBeVisible();
  expect(screen.getByRole("heading", { name: "Second" })).toBeVisible();
  expect(screen.getByText(project.id)).toBeVisible();
  expect(screen.getAllByText(/2023/).length).toBeGreaterThan(0);
  expect(screen.getByText("2 total")).toBeVisible();
  expect(document.body.textContent).not.toContain("C:\\private\\project");
  expect(document.body.textContent).not.toContain(forbiddenPathField);
  expect(screen.getAllByText("%USERPROFILE%\\Foundation")).toHaveLength(2);
});

it("falls back to Unknown for malformed timestamps", async () => {
  render(
    <ProjectsPage
      client={createFakeClient({
        listProjects: async () => [{ ...project, createdAtMs: Number.NaN, updatedAtMs: Infinity }],
      })}
    />,
  );
  expect(await screen.findByRole("heading", { name: "Foundation" })).toBeVisible();
  expect(screen.getAllByText("Unknown")).toHaveLength(2);
});

it("renders stable safe errors and retries project loading", async () => {
  const listProjects = vi
    .fn()
    .mockRejectedValueOnce(
      new FrontendError({
        code: "APP_STORAGE_UNAVAILABLE",
        message: "Secure local storage is unavailable.",
        severity: "error",
        retry: "immediate",
      }),
    )
    .mockResolvedValue([project]);
  render(<ProjectsPage client={createFakeClient({ listProjects })} />);
  expect(await screen.findByText("Secure local storage is unavailable.")).toBeVisible();
  expect(screen.getByText("Error code: APP_STORAGE_UNAVAILABLE")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("heading", { name: "Foundation" })).toBeVisible();
  await waitFor(() => expect(listProjects).toHaveBeenCalledTimes(2));
});

it("inspects a pasted path and requires confirmation before registration", async () => {
  const listProjects = vi.fn().mockResolvedValueOnce([]).mockResolvedValue([project]);
  const inspectProjectCandidate = vi.fn().mockResolvedValue({
    suggestedName: "Foundation",
    displayPath: "%USERPROFILE%\\Foundation",
    confirmationToken: "token",
    repositoryKind: "git",
    repositoryStatus: { clean: true, detachedHead: false, currentBranch: "main", headCommit: "a".repeat(40) },
  });
  const registerProject = vi.fn().mockResolvedValue(project);
  render(<ProjectsPage client={createFakeClient({ listProjects, inspectProjectCandidate, registerProject })} />);
  await screen.findByRole("heading", { name: "No projects" });
  fireEvent.change(screen.getByLabelText("Local directory"), { target: { value: "C:\\repo\\nested" } });
  fireEvent.click(screen.getByRole("button", { name: "Inspect" }));
  expect(await screen.findByText("Existing Git repository")).toBeVisible();
  expect(registerProject).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Confirm registration" }));
  expect(await screen.findByRole("heading", { name: "Foundation" })).toBeVisible();
  expect(registerProject).toHaveBeenCalledWith("C:\\repo\\nested", "token");
});

it("explains the exact non-Git mutation before approval", async () => {
  const createIsolationTask = vi.fn().mockResolvedValue({
    taskId: "task-id", projectId: project.id, taskState: "awaitingGitInitApproval", taskVersion: 1,
    isolationStatus: "awaitingGitInitApproval", branchIdentity: "ai-task/task-id",
    baseBranch: null, baseCommit: null, blocker: null,
  });
  render(<ProjectsPage client={createFakeClient({ listProjects: async () => [project], createIsolationTask })} />);
  await screen.findByRole("heading", { name: "Foundation" });
  fireEvent.click(screen.getByRole("button", { name: "Create isolated task" }));
  expect(await screen.findByRole("heading", { name: "Enter task requirements" })).toBeVisible();
  fireEvent.change(screen.getByLabelText("Requirements"), { target: { value: "Do something" } });
  fireEvent.change(screen.getByLabelText("Completion criteria"), { target: { value: "It works" } });
  fireEvent.change(screen.getByLabelText("Prohibited scope"), { target: { value: "Don't break it" } });
  fireEvent.click(screen.getByRole("button", { name: "Create task" }));
  expect(await screen.findByText(/git init/)).toBeVisible();
  expect(screen.getByText(/will not create or edit .gitignore or Git author settings/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Approve Git initialization" })).toBeVisible();
});

it("restores every active isolation state and suppresses duplicate task creation", async () => {
  for (const taskState of [
    "awaitingGitInitApproval",
    "projectValidated",
    "worktreeReady",
    "recoveryRequired",
  ] as const) {
    const isolation = restoredIsolation(taskState);
    const { unmount } = render(
      <ProjectsPage
        client={createFakeClient({
          listProjects: async () => [project],
          getActiveTask: async () => ({ taskId: isolation.taskId, acquiredAtMs: 1 }),
          getTask: async () => restoredTask(taskState),
          getTaskIsolation: async () => isolation,
        })}
      />,
    );
    expect(await screen.findByLabelText("Isolation for Foundation")).toHaveTextContent(taskState);
    expect(screen.queryByRole("button", { name: "Create isolated task" })).toBeNull();
    unmount();
  }
});

it("keeps existing task creation when no active task is restored", async () => {
  render(<ProjectsPage client={createFakeClient({ listProjects: async () => [project] })} />);
  expect(await screen.findByRole("button", { name: "Create isolated task" })).toBeVisible();
});

function eligibility(overrides: Partial<ProviderEligibilityDto> = {}): ProviderEligibilityDto {
  return {
    workKind: "planning",
    provider: "claude",
    capability: "supported",
    contract: "approved",
    eligible: true,
    stateAllowsWorkKind: true,
    blockingReasons: [],
    ...overrides,
  };
}

function firstOf(elements: readonly HTMLElement[]): HTMLElement {
  const [first] = elements;
  if (!first) throw new Error("expected at least one matching element");
  return first;
}

function renderActiveTask(taskState: TaskDto["state"], client: Partial<IpcClient> = {}) {
  const isolation = restoredIsolation(taskState);
  return {
    isolation,
    ...render(
      <ProjectsPage
        client={createFakeClient({
          listProjects: async () => [project],
          getActiveTask: async () => ({ taskId: isolation.taskId, acquiredAtMs: 1 }),
          getTask: async () => restoredTask(taskState),
          getTaskIsolation: async () => isolation,
          ...client,
        })}
      />,
    ),
  };
}

it("disables Claude Planning start with a fixed reason when eligibility reports one", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([
    eligibility({ eligible: false, capability: "unsupported", blockingReasons: ["capabilityUnsupported"] }),
  ]);
  renderActiveTask("worktreeReady", { getProviderEligibility });

  const startButton = await screen.findByRole("button", { name: "Start Claude Planning" });
  await waitFor(() => expect(startButton).toBeDisabled());
  expect(await screen.findByText("Claude Code CLI is not available or not logged in.")).toBeVisible();
});

it("confirms provider consent before starting Claude Planning when eligible", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility()]);
  const startClaudePlanning = vi.fn().mockResolvedValue(restoredTask("planning"));
  const { isolation } = renderActiveTask("worktreeReady", { getProviderEligibility, startClaudePlanning });

  const startButton = await screen.findByRole("button", { name: "Start Claude Planning" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);

  expect(await screen.findByRole("heading", { name: "Send task brief to Claude" })).toBeVisible();
  expect(startClaudePlanning).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Confirm and start" }));

  expect(await screen.findByText(/Claude Planning is analyzing/)).toBeVisible();
  expect(startClaudePlanning).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);
});

it("requests cancellation of an in-progress Claude Planning run", async () => {
  const cancelClaudePlanning = vi.fn().mockResolvedValue({ requested: true });
  const { isolation } = renderActiveTask("planning", { cancelClaudePlanning });

  const cancelButton = await screen.findByRole("button", { name: "Cancel planning" });
  fireEvent.click(cancelButton);

  await waitFor(() => expect(cancelClaudePlanning).toHaveBeenCalledWith(isolation.taskId));
});

it("shows a safe recovery notice when cancel finds no matching registry entry", async () => {
  const cancelClaudePlanning = vi.fn().mockResolvedValue({ requested: false });
  renderActiveTask("planning", { cancelClaudePlanning });

  const cancelButton = await screen.findByRole("button", { name: "Cancel planning" });
  fireEvent.click(cancelButton);

  expect(await screen.findByText(/No active Claude Planning execution was found/)).toBeVisible();
});

it("disables Claude Implementation start with a fixed reason when eligibility reports one", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([
    eligibility({
      workKind: "implementation",
      eligible: false,
      capability: "unsupported",
      blockingReasons: ["capabilityUnsupported"],
    }),
  ]);
  renderActiveTask("awaitingDesignApproval", { getProviderEligibility, getPlanningResult: async () => null });

  const startButton = await screen.findByRole("button", { name: "Start Claude Implementation" });
  await waitFor(() => expect(startButton).toBeDisabled());
  expect(await screen.findByText("Claude Code CLI is not available or not logged in.")).toBeVisible();
});

it("confirms provider consent before starting Claude Implementation when eligible", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  const startClaudeImplementation = vi.fn().mockResolvedValue(restoredTask("implementing"));
  const { isolation } = renderActiveTask("awaitingDesignApproval", {
    getProviderEligibility,
    startClaudeImplementation,
    getPlanningResult: async () => null,
  });

  const startButton = await screen.findByRole("button", { name: "Start Claude Implementation" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);

  expect(await screen.findByRole("heading", { name: "Send task brief and plan to Claude" })).toBeVisible();
  expect(startClaudeImplementation).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Confirm and start" }));

  expect(await screen.findByText(/Claude Implementation is applying changes/)).toBeVisible();
  expect(startClaudeImplementation).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);
});

const HIGH_RISK_CATEGORY_LABELS = [
  "Architecture change",
  "Database schema change",
  "Authentication or authorization change",
  "Security policy change",
  "External network behavior addition",
  "External data transmission addition",
  "Large-scale file move or deletion",
  "Public API or storage format change",
  "Operating system configuration change",
  "Administrator privileges required",
  "Breaking compatibility change",
  "Data migration",
  "Difficult-to-recover change",
];

it("shows all 13 fixed high-risk categories with per-category status in awaitingDesignApproval", async () => {
  const getHighRiskApprovalStatus = vi.fn().mockResolvedValue({ approved: false });
  renderActiveTask("awaitingDesignApproval", {
    getPlanningResult: async () => null,
    getHighRiskApprovalStatus,
  });

  expect(await screen.findByRole("heading", { name: "High-risk approval" })).toBeVisible();
  for (const label of HIGH_RISK_CATEGORY_LABELS) {
    expect(screen.getByText(label)).toBeVisible();
  }
  await waitFor(() => expect(getHighRiskApprovalStatus).toHaveBeenCalledTimes(13));
  expect(await screen.findAllByRole("button", { name: "Approve" })).toHaveLength(13);
});

it("does not show the high-risk approval panel outside awaitingDesignApproval", async () => {
  renderActiveTask("worktreeReady");
  await screen.findByRole("button", { name: "Start Claude Planning" });
  expect(screen.queryByRole("heading", { name: "High-risk approval" })).toBeNull();
});

it("does not call approveHighRiskOperation until the confirmation dialog is confirmed", async () => {
  const approveHighRiskOperation = vi.fn().mockResolvedValue({
    riskCategory: "architectureChange",
    approvedAtMs: 100,
  });
  renderActiveTask("awaitingDesignApproval", {
    getPlanningResult: async () => null,
    getHighRiskApprovalStatus: async () => ({ approved: false }),
    approveHighRiskOperation,
  });

  const approveButtons = await screen.findAllByRole("button", { name: "Approve" });
  fireEvent.click(firstOf(approveButtons));

  expect(await screen.findByRole("heading", { name: "Approve Architecture change" })).toBeVisible();
  expect(approveHighRiskOperation).not.toHaveBeenCalled();
});

it("shows only the fixed category label and non-execution/version-bound copy in the confirmation dialog", async () => {
  renderActiveTask("awaitingDesignApproval", {
    getPlanningResult: async () => null,
    getHighRiskApprovalStatus: async () => ({ approved: false }),
  });

  const approveButtons = await screen.findAllByRole("button", { name: "Approve" });
  fireEvent.click(firstOf(approveButtons));

  expect(await screen.findByRole("heading", { name: "Approve Architecture change" })).toBeVisible();
  expect(
    screen.getByText(
      "This approval applies only to the Architecture change effect category for this task's current version.",
    ),
  ).toBeVisible();
  expect(
    screen.getByText("Approval does not run any provider and does not change this task's status."),
  ).toBeVisible();
  expect(screen.getByText("If the version changes, this approval cannot be reused.")).toBeVisible();
});

it("updates only the confirmed category to approved after a successful approve, without touching task state or version", async () => {
  const approveHighRiskOperation = vi.fn().mockResolvedValue({
    riskCategory: "architectureChange",
    approvedAtMs: 100,
  });
  const { isolation } = renderActiveTask("awaitingDesignApproval", {
    getPlanningResult: async () => null,
    getHighRiskApprovalStatus: async () => ({ approved: false }),
    approveHighRiskOperation,
  });

  const approveButtons = await screen.findAllByRole("button", { name: "Approve" });
  fireEvent.click(firstOf(approveButtons));
  fireEvent.click(await screen.findByRole("button", { name: "Confirm approval" }));

  await waitFor(() =>
    expect(approveHighRiskOperation).toHaveBeenCalledWith(
      isolation.taskId,
      isolation.taskVersion,
      "architectureChange",
    ),
  );
  expect(await screen.findAllByText("Approved")).toHaveLength(1);
  expect(await screen.findAllByRole("button", { name: "Approve" })).toHaveLength(12);
  expect(screen.getByText("awaitingDesignApproval")).toBeVisible();
});

it("never surfaces raw plan text inside the high-risk approval panel or its confirmation dialog", async () => {
  renderActiveTask("awaitingDesignApproval", {
    getPlanningResult: async () => ({
      outcome: "completed",
      exitCode: 0,
      turnCount: 1,
      startedAtMs: 1,
      completedAtMs: 2,
      planText: "SECRET_PLAN_CONTENT_MARKER",
    }),
    getHighRiskApprovalStatus: async () => ({ approved: false }),
  });

  const approveButtons = await screen.findAllByRole("button", { name: "Approve" });
  fireEvent.click(firstOf(approveButtons));
  expect(await screen.findByRole("heading", { name: "Approve Architecture change" })).toBeVisible();
  expect(document.body.textContent).not.toContain("SECRET_PLAN_CONTENT_MARKER");
});

it("keeps the existing Claude Implementation start button and Context Package v1 actions working alongside the high-risk approval panel", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  const startClaudeImplementation = vi.fn().mockResolvedValue(restoredTask("implementing"));
  renderActiveTask("awaitingDesignApproval", {
    getProviderEligibility,
    startClaudeImplementation,
    getPlanningResult: async () => null,
    getHighRiskApprovalStatus: async () => ({ approved: false }),
  });

  expect(await screen.findByRole("heading", { name: "High-risk approval" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Prepare Context Package v1 consent" })).toBeVisible();
  const startButton = screen.getByRole("button", { name: "Start Claude Implementation" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);
  fireEvent.click(await screen.findByRole("button", { name: "Confirm and start" }));
  expect(await screen.findByText(/Claude Implementation is applying changes/)).toBeVisible();
});

it("requests cancellation of an in-progress Claude Implementation run", async () => {
  const cancelClaudeImplementation = vi.fn().mockResolvedValue({ requested: true });
  const { isolation } = renderActiveTask("implementing", { cancelClaudeImplementation });

  const cancelButton = await screen.findByRole("button", { name: "Cancel implementation" });
  fireEvent.click(cancelButton);

  await waitFor(() => expect(cancelClaudeImplementation).toHaveBeenCalledWith(isolation.taskId));
});

it("shows a safe recovery notice when Claude Implementation cancel finds no matching registry entry", async () => {
  const cancelClaudeImplementation = vi.fn().mockResolvedValue({ requested: false });
  renderActiveTask("implementing", { cancelClaudeImplementation });

  const cancelButton = await screen.findByRole("button", { name: "Cancel implementation" });
  fireEvent.click(cancelButton);

  expect(await screen.findByText(/No active Claude Implementation execution was found/)).toBeVisible();
});

it("shows safe status text for paused and recovery-required states without exposing internals", async () => {
  for (const [taskState, expectedText] of [
    ["paused", "The task is paused"],
    ["recoveryRequired", "result could not be confirmed"],
  ] as const) {
    const { unmount } = renderActiveTask(taskState);
    expect(await screen.findByText(new RegExp(expectedText))).toBeVisible();
    unmount();
  }
});

it("disables Claude Review start with a fixed reason when eligibility reports one", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([
    eligibility({
      workKind: "review",
      eligible: false,
      capability: "unsupported",
      blockingReasons: ["capabilityUnsupported"],
    }),
  ]);
  renderActiveTask("reviewing", { getProviderEligibility });

  const startButton = await screen.findByRole("button", { name: "Start Claude Review" });
  await waitFor(() => expect(startButton).toBeDisabled());
  expect(await screen.findByText("Claude Code CLI is not available or not logged in.")).toBeVisible();
});

it("confirms provider consent before starting Claude Review when eligible", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const startClaudeReview = vi.fn().mockResolvedValue(restoredTask("reviewing"));
  const { isolation } = renderActiveTask("reviewing", { getProviderEligibility, startClaudeReview });

  const startButton = await screen.findByRole("button", { name: "Start Claude Review" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);

  expect(await screen.findByRole("heading", { name: "Send task brief and diff to Claude" })).toBeVisible();
  expect(startClaudeReview).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Confirm and start" }));

  expect(await screen.findByText(/Claude Review is analyzing/)).toBeVisible();
  expect(startClaudeReview).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);
});

it("requests cancellation of an in-progress Claude Review run", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const startClaudeReview = vi.fn().mockResolvedValue(restoredTask("reviewing"));
  const cancelClaudeReview = vi.fn().mockResolvedValue({ requested: true });
  const { isolation } = renderActiveTask("reviewing", {
    getProviderEligibility,
    startClaudeReview,
    cancelClaudeReview,
  });

  const startButton = await screen.findByRole("button", { name: "Start Claude Review" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);
  fireEvent.click(await screen.findByRole("button", { name: "Confirm and start" }));
  await screen.findByText(/Claude Review is analyzing/);

  const cancelButton = await screen.findByRole("button", { name: "Cancel review" });
  fireEvent.click(cancelButton);

  await waitFor(() => expect(cancelClaudeReview).toHaveBeenCalledWith(isolation.taskId));
});

it("shows a safe recovery notice when Claude Review cancel finds no matching registry entry", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const startClaudeReview = vi.fn().mockResolvedValue(restoredTask("reviewing"));
  const cancelClaudeReview = vi.fn().mockResolvedValue({ requested: false });
  renderActiveTask("reviewing", { getProviderEligibility, startClaudeReview, cancelClaudeReview });

  const startButton = await screen.findByRole("button", { name: "Start Claude Review" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);
  fireEvent.click(await screen.findByRole("button", { name: "Confirm and start" }));
  await screen.findByText(/Claude Review is analyzing/);

  const cancelButton = await screen.findByRole("button", { name: "Cancel review" });
  fireEvent.click(cancelButton);

  expect(await screen.findByText(/No active Claude Review execution was found/)).toBeVisible();
});

it("shows read-only post-merge validation results for a completed task", async () => {
  const getPostMergeValidationResults = vi.fn().mockResolvedValue([
    {
      commandKind: "test",
      attemptSequence: 1,
      outcome: "success",
      exitCode: 0,
      safeSummary: "post-merge validation completed successfully",
      startedAtMs: 1,
      completedAtMs: 2,
    },
    {
      commandKind: "build",
      attemptSequence: 1,
      outcome: "success",
      exitCode: 0,
      safeSummary: "post-merge validation completed successfully",
      startedAtMs: 3,
      completedAtMs: 4,
    },
  ]);
  const { isolation } = renderActiveTask("completed", { getPostMergeValidationResults });

  expect(await screen.findByRole("heading", { name: "Post-merge validation results" })).toBeVisible();
  expect(screen.getByText("Test")).toBeVisible();
  expect(screen.getByText("Build")).toBeVisible();
  expect(screen.getAllByText(/Outcome: success/)).toHaveLength(2);
  expect(screen.getAllByText("post-merge validation completed successfully")).toHaveLength(2);
  expect(getPostMergeValidationResults).toHaveBeenCalledWith(isolation.taskId);
  expect(document.body.textContent).not.toContain("C:\\\\private");
});

it("shows a safe empty state for a RecoveryRequired task without post-merge results", async () => {
  const getPostMergeValidationResults = vi.fn().mockResolvedValue([]);
  const { isolation } = renderActiveTask("recoveryRequired", { getPostMergeValidationResults });

  expect(await screen.findByText("No post-merge validation results are available for this task.")).toBeVisible();
  expect(getPostMergeValidationResults).toHaveBeenCalledWith(isolation.taskId);
});

it("does not fetch post-merge results while validation is still running", async () => {
  const getPostMergeValidationResults = vi.fn().mockResolvedValue([]);
  renderActiveTask("postMergeTesting", { getPostMergeValidationResults });

  await screen.findByText(/Post-merge validation is pending/);
  await waitFor(() => expect(getPostMergeValidationResults).not.toHaveBeenCalled());
});

it("shows a safe error state when post-merge results cannot be loaded", async () => {
  const getPostMergeValidationResults = vi.fn().mockRejectedValue(new Error("raw repository failure"));
  renderActiveTask("completed", { getPostMergeValidationResults });

  expect(await screen.findByText("Post-merge validation results could not be loaded. Refresh to try again.")).toBeVisible();
  expect(document.body.textContent).not.toContain("raw repository failure");
});

it("shows the stored Claude Review result read-only while awaiting user diff approval", async () => {
  const getReviewResult = vi.fn().mockResolvedValue({
    outcome: "completed",
    exitCode: 0,
    turnCount: 3,
    startedAtMs: 1,
    completedAtMs: 2,
    reviewText: "The change matches the requirements and stays within scope.",
  });
  const { isolation } = renderActiveTask("awaitingUserDiffApproval", { getReviewResult });

  expect(await screen.findByText(/matches the requirements/)).toBeVisible();
  expect(getReviewResult).toHaveBeenCalledWith(isolation.taskId);
});

it("shows a safe empty state when no Claude Review result has been recorded", async () => {
  const getReviewResult = vi.fn().mockResolvedValue(null);
  renderActiveTask("awaitingUserDiffApproval", { getReviewResult });

  expect(await screen.findByText("No review is available for this task.")).toBeVisible();
});

it("shows a safe error state when the Claude Review result cannot be loaded, without a raw error", async () => {
  const getReviewResult = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_STORAGE_UNAVAILABLE",
      message: "Secure local storage is unavailable.",
      severity: "error",
      retry: "immediate",
    }),
  );
  renderActiveTask("awaitingUserDiffApproval", { getReviewResult });

  expect(await screen.findByText("The review could not be loaded. Refresh to try again.")).toBeVisible();
  expect(document.body.textContent).not.toContain("APP_STORAGE_UNAVAILABLE");
});

it("never fetches or displays the Claude Review result outside awaitingUserDiffApproval", async () => {
  const getReviewResult = vi.fn().mockResolvedValue({
    outcome: "completed",
    exitCode: 0,
    turnCount: 1,
    startedAtMs: 1,
    completedAtMs: 2,
    reviewText: "Should never surface here.",
  });
  for (const taskState of ["reviewing", "failed", "cancelled"] as const) {
    const { unmount } = renderActiveTask(taskState, { getReviewResult });
    await screen.findByLabelText("Isolation for Foundation");
    expect(getReviewResult).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain("Should never surface here.");
    unmount();
  }
});

it("shows the Review current diff action only while awaiting user diff approval", async () => {
  renderActiveTask("awaitingUserDiffApproval", { getReviewResult: async () => null });
  expect(await screen.findByRole("button", { name: "Review current diff" })).toBeVisible();
});

it("does not show the Review current diff action outside awaitingUserDiffApproval", async () => {
  for (const taskState of ["worktreeReady", "reviewing", "failed", "cancelled"] as const) {
    const { unmount } = renderActiveTask(taskState);
    await screen.findByLabelText("Isolation for Foundation");
    expect(screen.queryByRole("button", { name: "Review current diff" })).toBeNull();
    unmount();
  }
});

it("does not call getUserDiffForReview until the review modal is opened", async () => {
  const getUserDiffForReview = vi.fn().mockResolvedValue({
    diffText: "diff --git a/x b/x\n+line\n",
    diffContentHash: "a".repeat(64),
  });
  renderActiveTask("awaitingUserDiffApproval", {
    getReviewResult: async () => null,
    getUserDiffForReview,
  });

  await screen.findByRole("button", { name: "Review current diff" });
  expect(getUserDiffForReview).not.toHaveBeenCalled();
});

it("opens the review modal and displays the fetched diff only in local modal scope", async () => {
  const getUserDiffForReview = vi.fn().mockResolvedValue({
    diffText: "diff --git a/x b/x\n+added line\n",
    diffContentHash: "a".repeat(64),
  });
  const { isolation } = renderActiveTask("awaitingUserDiffApproval", {
    getReviewResult: async () => null,
    getUserDiffForReview,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Review current diff" }));

  expect(await screen.findByRole("heading", { name: "Review current diff" })).toBeVisible();
  expect(await screen.findByText(/\+added line/)).toBeVisible();
  expect(getUserDiffForReview).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);
});

it("shows a safe error state when the diff cannot be loaded, without exposing the raw error", async () => {
  const getUserDiffForReview = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_STORAGE_UNAVAILABLE",
      message: "Secure local storage is unavailable.",
      severity: "error",
      retry: "immediate",
    }),
  );
  renderActiveTask("awaitingUserDiffApproval", {
    getReviewResult: async () => null,
    getUserDiffForReview,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Review current diff" }));

  expect(
    await screen.findByText("The diff could not be loaded. Close and try again."),
  ).toBeVisible();
  expect(document.body.textContent).not.toContain("APP_STORAGE_UNAVAILABLE");
});

it("does not start merging before confirmation and sends only the digest to the combined command", async () => {
  const diffText = "diff --git a/x b/x\n+added line\n";
  const diffContentHash = "a".repeat(64);
  const getUserDiffForReview = vi.fn().mockResolvedValue({ diffText, diffContentHash });
  const approveUserDiffAndStartMerge = vi.fn().mockResolvedValue(restoredTask("merging"));
  const getProjectRootValidationApprovalStatus = vi.fn().mockResolvedValue({ testApproved: true, buildApproved: true });
  const { isolation } = renderActiveTask("awaitingUserDiffApproval", {
    getReviewResult: async () => null,
    getUserDiffForReview,
    getProjectRootValidationApprovalStatus,
    approveUserDiffAndStartMerge,
  });

  await screen.findByLabelText("Isolation for Foundation");
  fireEvent.click(await screen.findByRole("button", { name: "Review current diff" }));
  await screen.findByText(/\+added line/);
  expect(approveUserDiffAndStartMerge).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole("button", { name: "Approve and start merge" }));

  expect(await screen.findByText(/approved change is being committed and merged/)).toBeVisible();
  expect(approveUserDiffAndStartMerge).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion, diffContentHash);
  expect(getProjectRootValidationApprovalStatus).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);
  expect(approveUserDiffAndStartMerge).toHaveBeenCalledTimes(1);
  expect(approveUserDiffAndStartMerge.mock.calls[0]).not.toContain(diffText);
});

it("closing the review modal discards the diff so reopening fetches it again", async () => {
  const getUserDiffForReview = vi.fn().mockResolvedValue({
    diffText: "diff --git a/x b/x\n+line\n",
    diffContentHash: "a".repeat(64),
  });
  renderActiveTask("awaitingUserDiffApproval", {
    getReviewResult: async () => null,
    getUserDiffForReview,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Review current diff" }));
  await screen.findByText(/\+line/);
  fireEvent.click(screen.getByRole("button", { name: "Close" }));

  expect(await screen.findByRole("button", { name: "Review current diff" })).toBeVisible();
  expect(document.body.textContent).not.toContain("diff --git a/x b/x\n+line\n");

  fireEvent.click(screen.getByRole("button", { name: "Review current diff" }));
  await screen.findByText(/\+line/);
  await waitFor(() => expect(getUserDiffForReview).toHaveBeenCalledTimes(2));
});

it("never disturbs the existing Claude Review result panel or high-risk approval UI elsewhere", async () => {
  const getReviewResult = vi.fn().mockResolvedValue({
    outcome: "completed",
    exitCode: 0,
    turnCount: 3,
    startedAtMs: 1,
    completedAtMs: 2,
    reviewText: "The change matches the requirements and stays within scope.",
  });
  renderActiveTask("awaitingUserDiffApproval", { getReviewResult });

  expect(await screen.findByText(/matches the requirements/)).toBeVisible();
  expect(await screen.findByRole("button", { name: "Review current diff" })).toBeVisible();
});

it("shows safe status text for every state Claude Planning can end in", async () => {
  for (const [taskState, expectedText] of [
    ["awaitingDesignApproval", "awaiting design approval"],
    ["failed", "Claude Planning failed"],
    ["cancelled", "Claude Planning was cancelled"],
  ] as const) {
    const { unmount } = renderActiveTask(taskState);
    expect(await screen.findByText(new RegExp(expectedText))).toBeVisible();
    unmount();
  }
});

it("shows the stored Claude Planning result read-only while awaiting design approval", async () => {
  const getPlanningResult = vi.fn().mockResolvedValue({
    outcome: "completed",
    exitCode: 0,
    turnCount: 3,
    startedAtMs: 1,
    completedAtMs: 2,
    planText: "1. Add a CSV export button.\n2. Wire it to the export service.",
  });
  const { isolation } = renderActiveTask("awaitingDesignApproval", { getPlanningResult });

  expect(await screen.findByText(/Add a CSV export button/)).toBeVisible();
  expect(getPlanningResult).toHaveBeenCalledWith(isolation.taskId);
});

it("shows a safe empty state when no Claude Planning result has been recorded", async () => {
  const getPlanningResult = vi.fn().mockResolvedValue(null);
  renderActiveTask("awaitingDesignApproval", { getPlanningResult });

  expect(await screen.findByText("No plan is available for this task.")).toBeVisible();
});

it("shows a safe error state when the Claude Planning result cannot be loaded, without a raw error", async () => {
  const getPlanningResult = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_STORAGE_UNAVAILABLE",
      message: "Secure local storage is unavailable.",
      severity: "error",
      retry: "immediate",
    }),
  );
  renderActiveTask("awaitingDesignApproval", { getPlanningResult });

  expect(await screen.findByText("The plan could not be loaded. Refresh to try again.")).toBeVisible();
  expect(document.body.textContent).not.toContain("APP_STORAGE_UNAVAILABLE");
});

it("never fetches or displays the Claude Planning result outside awaitingDesignApproval", async () => {
  const getPlanningResult = vi.fn().mockResolvedValue({
    outcome: "completed",
    exitCode: 0,
    turnCount: 1,
    startedAtMs: 1,
    completedAtMs: 2,
    planText: "Should never surface here.",
  });
  for (const taskState of ["planning", "failed", "cancelled"] as const) {
    const { unmount } = renderActiveTask(taskState, { getPlanningResult });
    await screen.findByLabelText("Isolation for Foundation");
    expect(getPlanningResult).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain("Should never surface here.");
    unmount();
  }
});

it("does not present an active-task restore failure as an empty successful state", async () => {
  render(
    <ProjectsPage
      client={createFakeClient({
        listProjects: async () => [project],
        getActiveTask: async () => {
          throw new FrontendError({
            code: "APP_STORAGE_UNAVAILABLE",
            message: "Secure local storage is unavailable.",
            severity: "error",
            retry: "immediate",
          });
        },
      })}
    />,
  );
  expect(await screen.findByText("Secure local storage is unavailable.")).toBeVisible();
  expect(screen.queryByRole("heading", { name: "No projects" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Create isolated task" })).toBeNull();
});

it("shows discovered Cargo validation candidates and lets the user enter a tool path", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([
    { kind: "format", label: "Format (cargo fmt --check)" },
    { kind: "test", label: "Test (cargo test)" },
  ]);
  const getValidationCommandApprovalStatus = vi.fn().mockResolvedValue({ approvedKinds: [] });
  renderActiveTask("testing", { getValidationCommandCandidates, getValidationCommandApprovalStatus });

  expect(await screen.findByLabelText("Testing validation")).toBeVisible();
  expect(await screen.findByText("Format (cargo fmt --check)")).toBeVisible();
  expect(screen.getByText("Test (cargo test)")).toBeVisible();
  const pathInput = screen.getByLabelText("Cargo executable path") as HTMLInputElement;
  fireEvent.change(pathInput, { target: { value: "C:\\tools\\cargo\\bin\\cargo.exe" } });
  expect(pathInput.value).toBe("C:\\tools\\cargo\\bin\\cargo.exe");
});

it("disables Start and shows a safe notice until at least one validation command is approved", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([
    { kind: "test", label: "Test (cargo test)" },
  ]);
  const getValidationCommandApprovalStatus = vi.fn().mockResolvedValue({ approvedKinds: [] });
  renderActiveTask("testing", { getValidationCommandCandidates, getValidationCommandApprovalStatus });

  const startButton = await screen.findByRole("button", { name: "Start approved validation" });
  expect(startButton).toBeDisabled();
  expect(screen.getByText("Approve at least one validation command before starting.")).toBeVisible();
});

it("approves the selected validation commands and then enables Start, without auto-starting", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([
    { kind: "test", label: "Test (cargo test)" },
  ]);
  const getValidationCommandApprovalStatus = vi.fn().mockResolvedValue({ approvedKinds: [] });
  const approveValidationCommand = vi.fn().mockResolvedValue({ approvedKinds: ["test"] });
  const startValidationTesting = vi.fn().mockResolvedValue(restoredTask("testing"));
  const { isolation } = renderActiveTask("testing", {
    getValidationCommandCandidates,
    getValidationCommandApprovalStatus,
    approveValidationCommand,
    startValidationTesting,
  });

  fireEvent.click(await screen.findByLabelText("Test (cargo test)"));
  fireEvent.change(screen.getByLabelText("Cargo executable path"), {
    target: { value: "C:\\tools\\cargo\\bin\\cargo.exe" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Approve selected validation commands" }));

  await waitFor(() =>
    expect(approveValidationCommand).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion, {
      kinds: ["test"],
      executablePath: "C:\\tools\\cargo\\bin\\cargo.exe",
      cargoHomePath: null,
      rustupHomePath: null,
    }),
  );
  expect(startValidationTesting).not.toHaveBeenCalled();

  const startButton = await screen.findByRole("button", { name: "Start approved validation" });
  await waitFor(() => expect(startButton).not.toBeDisabled());

  fireEvent.click(startButton);
  await waitFor(() =>
    expect(startValidationTesting).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion),
  );
  expect(await screen.findByText(/Validation is running/)).toBeVisible();
});

it("never redisplays the entered executable path after an approval attempt", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([
    { kind: "test", label: "Test (cargo test)" },
  ]);
  const getValidationCommandApprovalStatus = vi.fn().mockResolvedValue({ approvedKinds: [] });
  const approveValidationCommand = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_INVALID_INPUT",
      message: "The provided input was invalid.",
      severity: "error",
      retry: "afterUserAction",
    }),
  );
  renderActiveTask("testing", {
    getValidationCommandCandidates,
    getValidationCommandApprovalStatus,
    approveValidationCommand,
  });

  fireEvent.click(await screen.findByLabelText("Test (cargo test)"));
  fireEvent.change(screen.getByLabelText("Cargo executable path"), {
    target: { value: "C:\\secret\\cargo.exe" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Approve selected validation commands" }));

  expect(await screen.findByText("The provided input was invalid.")).toBeVisible();
  await waitFor(() =>
    expect((screen.getByLabelText("Cargo executable path") as HTMLInputElement).value).toBe(""),
  );
});

it("requests cancellation of an in-progress Testing validation run", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([]);
  const getValidationCommandApprovalStatus = vi.fn().mockResolvedValue({ approvedKinds: ["test"] });
  const startValidationTesting = vi.fn().mockResolvedValue(restoredTask("testing"));
  const cancelValidationTesting = vi.fn().mockResolvedValue({ requested: true });
  const { isolation } = renderActiveTask("testing", {
    getValidationCommandCandidates,
    getValidationCommandApprovalStatus,
    startValidationTesting,
    cancelValidationTesting,
  });

  const startButton = await screen.findByRole("button", { name: "Start approved validation" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);

  const cancelButton = await screen.findByRole("button", { name: "Cancel validation" });
  fireEvent.click(cancelButton);

  await waitFor(() => expect(cancelValidationTesting).toHaveBeenCalledWith(isolation.taskId));
});

it("shows a safe recovery notice when Testing cancel finds no matching registry entry", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([]);
  const getValidationCommandApprovalStatus = vi.fn().mockResolvedValue({ approvedKinds: ["test"] });
  const startValidationTesting = vi.fn().mockResolvedValue(restoredTask("testing"));
  const cancelValidationTesting = vi.fn().mockResolvedValue({ requested: false });
  renderActiveTask("testing", {
    getValidationCommandCandidates,
    getValidationCommandApprovalStatus,
    startValidationTesting,
    cancelValidationTesting,
  });

  const startButton = await screen.findByRole("button", { name: "Start approved validation" });
  await waitFor(() => expect(startButton).not.toBeDisabled());
  fireEvent.click(startButton);
  fireEvent.click(await screen.findByRole("button", { name: "Cancel validation" }));

  expect(await screen.findByText(/No active validation run was found/)).toBeVisible();
});

it("shows a safe error state when validation candidates or approval status cannot be loaded", async () => {
  const getValidationCommandCandidates = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_STORAGE_UNAVAILABLE",
      message: "Secure local storage is unavailable.",
      severity: "error",
      retry: "immediate",
    }),
  );
  renderActiveTask("testing", { getValidationCommandCandidates });

  expect(
    await screen.findByText("Validation commands could not be loaded. Refresh to try again."),
  ).toBeVisible();
  expect(document.body.textContent).not.toContain("APP_STORAGE_UNAVAILABLE");
});

it("shows the Context Package v1 preparation action at each work kind's own gate state", async () => {
  for (const [taskState, workKind] of [
    ["worktreeReady", "planning"],
    ["awaitingDesignApproval", "implementation"],
    ["reviewing", "review"],
  ] as const) {
    const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind })]);
    const extra = taskState === "awaitingDesignApproval" ? { getPlanningResult: async () => null } : {};
    const { unmount } = renderActiveTask(taskState, { getProviderEligibility, ...extra });
    const prepareButton = await screen.findByRole("button", { name: "Prepare Context Package v1 consent" });
    await waitFor(() => expect(prepareButton).not.toBeDisabled());
    unmount();
  }
});

it("never shows the Context Package v1 preparation action outside its work kind's gate state", async () => {
  renderActiveTask("planning");
  await screen.findByLabelText("Isolation for Foundation");
  expect(screen.queryByRole("button", { name: "Prepare Context Package v1 consent" })).toBeNull();
});

it("disables the Context Package v1 preparation action using the existing eligibility gate", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([
    eligibility({ eligible: false, capability: "unsupported", blockingReasons: ["capabilityUnsupported"] }),
  ]);
  renderActiveTask("worktreeReady", { getProviderEligibility });

  const prepareButton = await screen.findByRole("button", { name: "Prepare Context Package v1 consent" });
  await waitFor(() => expect(prepareButton).toBeDisabled());
  expect(screen.getByRole("button", { name: "Start Claude Planning" })).toBeDisabled();
});

it("disables the Context Package v1 Planning activation action with a fixed reason when readiness is not yet prepared", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility()]);
  const getContextPackagePlanningReadiness = vi.fn().mockResolvedValue({ ready: false });
  renderActiveTask("worktreeReady", { getProviderEligibility, getContextPackagePlanningReadiness });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Planning (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).toBeDisabled());
  expect(await screen.findByText("Prepare Context Package v1 consent first.")).toBeVisible();
  // The existing Legacy action and the preparation action must remain
  // available alongside the new one, unaffected by CPv1 readiness.
  expect(screen.getByRole("button", { name: "Start Claude Planning" })).not.toBeDisabled();
  expect(
    screen.getByRole("button", { name: "Prepare Context Package v1 consent" }),
  ).not.toBeDisabled();
});

it("enables the Context Package v1 Planning activation action once readiness reports ready", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility()]);
  const getContextPackagePlanningReadiness = vi.fn().mockResolvedValue({ ready: true });
  renderActiveTask("worktreeReady", { getProviderEligibility, getContextPackagePlanningReadiness });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Planning (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).not.toBeDisabled());
  expect(screen.queryByText("Prepare Context Package v1 consent first.")).toBeNull();
});

it("treats a readiness fetch error as not-ready rather than assuming enabled", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility()]);
  const getContextPackagePlanningReadiness = vi.fn().mockRejectedValue(new Error("boom"));
  renderActiveTask("worktreeReady", { getProviderEligibility, getContextPackagePlanningReadiness });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Planning (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).toBeDisabled());
  expect(
    await screen.findByText("Context Package v1 readiness could not be loaded. Refresh to try again."),
  ).toBeVisible();
});

it("activating Context Package v1 Planning calls only the new start command, never the legacy one, and switches to the existing Planning UI", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility()]);
  const getContextPackagePlanningReadiness = vi.fn().mockResolvedValue({ ready: true });
  const startClaudePlanningContextPackage = vi.fn().mockResolvedValue(restoredTask("planning"));
  const startClaudePlanning = vi.fn();
  const { isolation } = renderActiveTask("worktreeReady", {
    getProviderEligibility,
    getContextPackagePlanningReadiness,
    startClaudePlanningContextPackage,
    startClaudePlanning,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Planning (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).not.toBeDisabled());
  fireEvent.click(activateButton);

  await waitFor(() =>
    expect(startClaudePlanningContextPackage).toHaveBeenCalledWith(
      isolation.taskId,
      isolation.taskVersion,
    ),
  );
  expect(startClaudePlanning).not.toHaveBeenCalled();
  // No re-consent dialog: activation must not reuse the LegacyPhase4
  // consent screen -- the CPv1 data-scope consent was already recorded by
  // the separate "Prepare" step.
  expect(screen.queryByRole("heading", { name: "Send task brief to Claude" })).toBeNull();
  expect(await screen.findByText(/Claude Planning is analyzing/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Cancel planning" })).toBeVisible();
});

it("shows only permitted data categories and a non-execution notice in the Review Context Package v1 dialog, without calling the IPC method before confirm", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const prepareReviewContextPackage = vi.fn();
  renderActiveTask("reviewing", { getProviderEligibility, prepareReviewContextPackage });

  const prepareButton = await screen.findByRole("button", { name: "Prepare Context Package v1 consent" });
  await waitFor(() => expect(prepareButton).not.toBeDisabled());
  fireEvent.click(prepareButton);

  expect(
    await screen.findByRole("heading", { name: "Prepare Context Package v1 consent" }),
  ).toBeVisible();
  expect(screen.getByText(/current Git diff/)).toBeVisible();
  expect(screen.getByText(/does not start Claude/)).toBeVisible();
  expect(screen.getByText(/Actual values are never shown here/)).toBeVisible();
  expect(prepareReviewContextPackage).not.toHaveBeenCalled();
});

it("disables the Context Package v1 Review activation action until readiness is confirmed", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const getContextPackageReviewReadiness = vi.fn().mockResolvedValue({ ready: false });
  renderActiveTask("reviewing", {
    getProviderEligibility,
    getContextPackageReviewReadiness,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Review (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).toBeDisabled());
  expect(await screen.findByText("Prepare Context Package v1 consent first.")).toBeVisible();
  // The existing Legacy action and the preparation action must remain
  // available alongside the new one, unaffected by CPv1 readiness.
  expect(screen.getByRole("button", { name: "Start Claude Review" })).not.toBeDisabled();
  expect(
    screen.getByRole("button", { name: "Prepare Context Package v1 consent" }),
  ).not.toBeDisabled();
});

it("enables the Context Package v1 Review activation action once readiness reports ready", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const getContextPackageReviewReadiness = vi.fn().mockResolvedValue({ ready: true });
  renderActiveTask("reviewing", {
    getProviderEligibility,
    getContextPackageReviewReadiness,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Review (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).not.toBeDisabled());
  expect(screen.queryByText("Prepare Context Package v1 consent first.")).toBeNull();
});

it("treats a Review readiness fetch error as not-ready rather than assuming enabled", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const getContextPackageReviewReadiness = vi.fn().mockRejectedValue(new Error("boom"));
  renderActiveTask("reviewing", {
    getProviderEligibility,
    getContextPackageReviewReadiness,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Review (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).toBeDisabled());
  expect(
    await screen.findByText("Context Package v1 readiness could not be loaded. Refresh to try again."),
  ).toBeVisible();
});

it("activating Context Package v1 Review calls only the new start command, never the legacy one, and switches to the existing Reviewing progress UI", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const getContextPackageReviewReadiness = vi.fn().mockResolvedValue({ ready: true });
  const startClaudeReviewContextPackage = vi.fn().mockResolvedValue(restoredTask("reviewing"));
  const startClaudeReview = vi.fn();
  const { isolation } = renderActiveTask("reviewing", {
    getProviderEligibility,
    getContextPackageReviewReadiness,
    startClaudeReviewContextPackage,
    startClaudeReview,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Review (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).not.toBeDisabled());
  fireEvent.click(activateButton);

  await waitFor(() =>
    expect(startClaudeReviewContextPackage).toHaveBeenCalledWith(
      isolation.taskId,
      isolation.taskVersion,
    ),
  );
  expect(startClaudeReview).not.toHaveBeenCalled();
  // No re-consent dialog: activation must not reuse the LegacyPhase4
  // consent screen -- the CPv1 data-scope consent was already recorded by
  // the separate "Prepare" step.
  expect(screen.queryByRole("heading", { name: "Send task brief and diff to Claude" })).toBeNull();
  expect(await screen.findByText(/Claude Review is analyzing/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Cancel review" })).toBeVisible();
});

it("shows the approved plan category in the Implementation Context Package v1 dialog", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  renderActiveTask("awaitingDesignApproval", { getProviderEligibility, getPlanningResult: async () => null });

  const prepareButton = await screen.findByRole("button", { name: "Prepare Context Package v1 consent" });
  await waitFor(() => expect(prepareButton).not.toBeDisabled());
  fireEvent.click(prepareButton);

  expect(await screen.findByText(/the approved plan/)).toBeVisible();
});

it("confirms Review Context Package v1 preparation without changing task state or calling the real start command", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "review" })]);
  const prepareReviewContextPackage = vi.fn().mockResolvedValue({
    workKind: "review",
    dataScope: "contextPackageV1",
    consentedAtMs: 200,
    manifestCreatedAtMs: 210,
  });
  const startClaudeReview = vi.fn();
  const { isolation } = renderActiveTask("reviewing", {
    getProviderEligibility,
    prepareReviewContextPackage,
    startClaudeReview,
  });

  const prepareButton = await screen.findByRole("button", { name: "Prepare Context Package v1 consent" });
  await waitFor(() => expect(prepareButton).not.toBeDisabled());
  fireEvent.click(prepareButton);
  fireEvent.click(await screen.findByRole("button", { name: "Confirm preparation" }));

  await waitFor(() =>
    expect(prepareReviewContextPackage).toHaveBeenCalledWith(
      isolation.taskId,
      isolation.taskVersion,
    ),
  );
  expect(startClaudeReview).not.toHaveBeenCalled();
  expect(
    await screen.findByText(/Claude was not started and this task's status is unchanged/),
  ).toBeVisible();
  expect(await screen.findByLabelText("Isolation for Foundation")).toHaveTextContent("reviewing");
  expect(screen.getByRole("button", { name: "Start Claude Review" })).toBeVisible();
});

it("disables the Context Package v1 Implementation activation action with a fixed reason when readiness is not yet prepared", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  const getContextPackageImplementationReadiness = vi.fn().mockResolvedValue({ ready: false });
  renderActiveTask("awaitingDesignApproval", {
    getProviderEligibility,
    getPlanningResult: async () => null,
    getContextPackageImplementationReadiness,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Implementation (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).toBeDisabled());
  expect(await screen.findByText("Prepare Context Package v1 consent first.")).toBeVisible();
  // The existing Legacy action and the preparation action must remain
  // available alongside the new one, unaffected by CPv1 readiness.
  expect(screen.getByRole("button", { name: "Start Claude Implementation" })).not.toBeDisabled();
  expect(
    screen.getByRole("button", { name: "Prepare Context Package v1 consent" }),
  ).not.toBeDisabled();
});

it("enables the Context Package v1 Implementation activation action once readiness reports ready", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  const getContextPackageImplementationReadiness = vi.fn().mockResolvedValue({ ready: true });
  renderActiveTask("awaitingDesignApproval", {
    getProviderEligibility,
    getPlanningResult: async () => null,
    getContextPackageImplementationReadiness,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Implementation (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).not.toBeDisabled());
  expect(screen.queryByText("Prepare Context Package v1 consent first.")).toBeNull();
});

it("treats an Implementation readiness fetch error as not-ready rather than assuming enabled", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  const getContextPackageImplementationReadiness = vi.fn().mockRejectedValue(new Error("boom"));
  renderActiveTask("awaitingDesignApproval", {
    getProviderEligibility,
    getPlanningResult: async () => null,
    getContextPackageImplementationReadiness,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Implementation (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).toBeDisabled());
  expect(
    await screen.findByText("Context Package v1 readiness could not be loaded. Refresh to try again."),
  ).toBeVisible();
});

it("activating Context Package v1 Implementation calls only the new start command, never the legacy one, and switches to the existing Implementing UI", async () => {
  const getProviderEligibility = vi.fn().mockResolvedValue([eligibility({ workKind: "implementation" })]);
  const getContextPackageImplementationReadiness = vi.fn().mockResolvedValue({ ready: true });
  const startClaudeImplementationContextPackage = vi.fn().mockResolvedValue(restoredTask("implementing"));
  const startClaudeImplementation = vi.fn();
  const { isolation } = renderActiveTask("awaitingDesignApproval", {
    getProviderEligibility,
    getPlanningResult: async () => null,
    getContextPackageImplementationReadiness,
    startClaudeImplementationContextPackage,
    startClaudeImplementation,
  });

  const activateButton = await screen.findByRole("button", {
    name: "Start Claude Implementation (Context Package v1)",
  });
  await waitFor(() => expect(activateButton).not.toBeDisabled());
  fireEvent.click(activateButton);

  await waitFor(() =>
    expect(startClaudeImplementationContextPackage).toHaveBeenCalledWith(
      isolation.taskId,
      isolation.taskVersion,
    ),
  );
  expect(startClaudeImplementation).not.toHaveBeenCalled();
  // No re-consent dialog: activation must not reuse the LegacyPhase4
  // consent screen -- the CPv1 data-scope consent was already recorded by
  // the separate "Prepare" step.
  expect(
    screen.queryByRole("heading", { name: "Send task brief and plan to Claude" }),
  ).toBeNull();
  expect(await screen.findByText(/Claude Implementation is applying changes/)).toBeVisible();
  expect(screen.getByRole("button", { name: "Cancel implementation" })).toBeVisible();
});

it("never renders Testing validation controls outside the testing state", async () => {
  const getValidationCommandCandidates = vi.fn().mockResolvedValue([
    { kind: "test", label: "Test (cargo test)" },
  ]);
  for (const taskState of ["planning", "implementing", "paused", "reviewing"] as const) {
    const { unmount } = renderActiveTask(taskState, { getValidationCommandCandidates });
    await screen.findByLabelText("Isolation for Foundation");
    expect(getValidationCommandCandidates).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Testing validation")).toBeNull();
    unmount();
  }
});

it("shows merge progress and safe terminal merge guidance without a cancel action", async () => {
  const { unmount } = renderActiveTask("merging");
  expect(await screen.findByText(/approved change is being committed and merged/)).toBeVisible();
  expect(screen.queryByRole("button", { name: /cancel merge/i })).toBeNull();
  unmount();

  renderActiveTask("postMergeTesting");
  expect(await screen.findByText("The merge completed. Post-merge validation is pending.")).toBeVisible();
});

it("shows conflict and recovery messages without exposing Git command output", async () => {
  const { unmount } = renderActiveTask("mergeConflict");
  expect(await screen.findByText(/No merge conflict inspection is available/)).toBeVisible();
  expect(document.body.textContent).not.toContain("fatal:");
  unmount();

  renderActiveTask("recoveryRequired");
  expect(await screen.findByText("The task result could not be confirmed. Review the repository safely before proceeding.")).toBeVisible();
});

it("shows only typed merge-conflict counts and fixed safe messages", async () => {
  const counts = {
    total: 2,
    bothModified: 1,
    bothAdded: 0,
    bothDeleted: 0,
    addedByUs: 0,
    addedByThem: 1,
    deletedByUs: 0,
    deletedByThem: 0,
  } as const;
  const getMergeConflictInspection = vi.fn().mockResolvedValue({
    outcome: "confirmedUnresolved",
    counts,
  });
  const { isolation, unmount } = renderActiveTask("mergeConflict", { getMergeConflictInspection });
  expect(await screen.findByText("Git reported merge conflicts. ChatOMS did not modify or resolve them.")).toBeVisible();
  expect(screen.getByText("Total: 2")).toBeVisible();
  expect(screen.getByText("Both modified: 1")).toBeVisible();
  expect(screen.getByText("Added by them: 1")).toBeVisible();
  expect(document.body.textContent).not.toContain("tracked.txt");
  expect(getMergeConflictInspection).toHaveBeenCalledWith(isolation.taskId);
  expect(screen.queryByRole("button", { name: "Refresh isolation" })).toBeNull();
  unmount();

  for (const outcome of [
    ["resolvedPendingConfirmation", "Git no longer reports unmerged entries, but ChatOMS has not confirmed or completed the merge."],
    ["inconsistent", "The saved task and current Git merge state do not match. No merge action was attempted."],
    ["unavailable", "The merge conflict state could not be verified safely. No merge action was attempted."],
  ] as const) {
    const { unmount: unmountOutcome } = renderActiveTask("mergeConflict", {
      getMergeConflictInspection: async () => ({ outcome: outcome[0], counts }),
    });
    expect(await screen.findByText(outcome[1])).toBeVisible();
    unmountOutcome();
  }
});

it("does not inspect Git for non-conflict task states", async () => {
  const getMergeConflictInspection = vi.fn();
  for (const taskState of ["merging", "postMergeTesting", "completed", "recoveryRequired"] as const) {
    const { unmount } = renderActiveTask(taskState, { getMergeConflictInspection });
    await screen.findByLabelText("Isolation for Foundation");
    await waitFor(() => expect(getMergeConflictInspection).not.toHaveBeenCalled());
    unmount();
  }
});

const noConflictCounts = {
  total: 0,
  bothModified: 0,
  bothAdded: 0,
  bothDeleted: 0,
  addedByUs: 0,
  addedByThem: 0,
  deletedByUs: 0,
  deletedByThem: 0,
} as const;

it("shows the fixed staged-resolution confirmation action only when Git reports it resolved", async () => {
  const { unmount } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
  });
  expect(await screen.findByRole("button", { name: "Confirm the staged merge resolution" })).toBeVisible();
  unmount();

  for (const outcome of ["confirmedUnresolved", "inconsistent", "unavailable"] as const) {
    const { unmount: unmountOutcome } = renderActiveTask("mergeConflict", {
      getMergeConflictInspection: async () => ({ outcome, counts: noConflictCounts }),
    });
    await screen.findByLabelText("Isolation for Foundation");
    expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
    unmountOutcome();
  }

  const { unmount: unmountLoading } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: () => new Promise(() => {}),
  });
  await screen.findByText("Checking the Git merge state safely…");
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  unmountLoading();

  const { unmount: unmountError } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => { throw new Error("boom"); },
  });
  await screen.findByText("The merge conflict state could not be loaded safely.");
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  unmountError();
});

it("requires an explicit checkbox before confirming, prevents duplicate submission, and reflects Merging on success", async () => {
  const confirmManualResolutionAndStartMergeContinue = vi.fn().mockResolvedValue(restoredTask("merging"));
  const { isolation } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    confirmManualResolutionAndStartMergeContinue,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Confirm the staged merge resolution" }));
  expect(await screen.findByRole("heading", { name: "Confirm the staged merge resolution" })).toBeVisible();
  expect(
    screen.getByText(/Git reports no unresolved entries\. Continuing will create a merge commit from the currently staged resolution/),
  ).toBeVisible();
  expect(screen.getByText("This confirmation is separate from the earlier task diff approval.")).toBeVisible();
  expect(document.body.textContent).not.toContain("fatal:");
  expect(screen.queryByRole("button", { name: /abort/i })).toBeNull();
  expect(screen.queryByRole("button", { name: /retry/i })).toBeNull();
  expect(screen.queryByRole("button", { name: /automatic/i })).toBeNull();

  const confirmButton = screen.getByRole("button", { name: "Confirm and continue" });
  expect(confirmButton).toBeDisabled();
  expect(confirmManualResolutionAndStartMergeContinue).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole("checkbox", { name: /I reviewed the staged merge resolution/ }));
  expect(confirmButton).not.toBeDisabled();

  fireEvent.click(confirmButton);
  fireEvent.click(confirmButton);

  await waitFor(() => expect(confirmManualResolutionAndStartMergeContinue).toHaveBeenCalledTimes(1));
  expect(confirmManualResolutionAndStartMergeContinue).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);
  expect(await screen.findByText(/approved change is being committed and merged/)).toBeVisible();
  expect(screen.queryByRole("button", { name: /cancel merge/i })).toBeNull();
});

it("keeps the confirmation dialog open and does not advance the task when the combined command fails", async () => {
  const confirmManualResolutionAndStartMergeContinue = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_CONFLICT",
      message: "The staged resolution no longer matches the confirmed digest.",
      severity: "error",
      retry: "afterStateRefresh",
    }),
  );
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    confirmManualResolutionAndStartMergeContinue,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Confirm the staged merge resolution" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /I reviewed the staged merge resolution/ }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm and continue" }));

  expect(await screen.findByText("The staged resolution no longer matches the confirmed digest.")).toBeVisible();
  expect(screen.getByRole("heading", { name: "Confirm the staged merge resolution" })).toBeVisible();
  expect(screen.getByRole("button", { name: "Confirm and continue" })).toBeVisible();
  expect(confirmManualResolutionAndStartMergeContinue).toHaveBeenCalledTimes(1);
});

it("shows the fixed merge-abort action only for the three approved inspection outcomes", async () => {
  for (const outcome of [
    "confirmedUnresolved",
    "resolvedPendingConfirmation",
    "restoredPendingAbortConfirmation",
  ] as const) {
    const { unmount } = renderActiveTask("mergeConflict", {
      getMergeConflictInspection: async () => ({ outcome, counts: noConflictCounts }),
    });
    expect(await screen.findByRole("button", { name: "Abort the in-progress merge" })).toBeVisible();
    unmount();
  }

  for (const outcome of ["inconsistent", "unavailable"] as const) {
    const { unmount } = renderActiveTask("mergeConflict", {
      getMergeConflictInspection: async () => ({ outcome, counts: noConflictCounts }),
    });
    await screen.findByLabelText("Isolation for Foundation");
    expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
    unmount();
  }

  const { unmount: unmountLoading } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: () => new Promise(() => {}),
  });
  await screen.findByText("Checking the Git merge state safely…");
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
  unmountLoading();
});

it("shows a safe restored-repository message for restoredPendingAbortConfirmation without raw Git content", async () => {
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "restoredPendingAbortConfirmation", counts: noConflictCounts }),
  });
  expect(
    await screen.findByText(/Git reports no merge in progress, and the original checkout already matches the base state/),
  ).toBeVisible();
  expect(document.body.textContent).not.toContain("fatal:");
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
});

it("requires an explicit checkbox before confirming a merge abort, prevents duplicate submission, and hides the continue action once started", async () => {
  const confirmMergeAbortAndStart = vi.fn().mockResolvedValue({ started: true });
  const { isolation } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    confirmMergeAbortAndStart,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Abort the in-progress merge" }));
  expect(await screen.findByRole("heading", { name: "Abort the in-progress merge" })).toBeVisible();
  expect(
    screen.getByText(/This discards the staged merge resolution in the original checkout and restores it to the base commit/),
  ).toBeVisible();
  expect(
    screen.getByText("This approval is separate from the earlier task diff approval and from any staged-resolution confirmation."),
  ).toBeVisible();
  expect(document.body.textContent).not.toContain("fatal:");
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();

  const confirmButton = screen.getByRole("button", { name: "Confirm abort" });
  expect(confirmButton).toBeDisabled();
  expect(confirmMergeAbortAndStart).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole("checkbox", { name: "I approve aborting the in-progress merge and cancelling this task." }));
  expect(confirmButton).not.toBeDisabled();

  fireEvent.click(confirmButton);
  fireEvent.click(confirmButton);

  await waitFor(() => expect(confirmMergeAbortAndStart).toHaveBeenCalledTimes(1));
  expect(confirmMergeAbortAndStart).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion);

  // Task state stays `mergeConflict` while the background abort runs, so
  // both the continue and (a second) abort action must be replaced by a
  // status message rather than remaining independently clickable.
  expect(await screen.findByText("A merge action is in progress for this task. This status updates automatically.")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
});

// Phase 5f-3b: the merge-conflict actions are gated on the Tauri runtime's
// shared `MergeConflictWriteLock`, not on this page's local state. A task
// stays `mergeConflict` for the whole duration of a background
// merge-continue or merge-abort write, so "still `mergeConflict` after
// another polling tick" is not evidence that the write finished.

it("offers no merge action while the runtime reports a merge-conflict write running, even for an action-eligible outcome", async () => {
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    getMergeConflictWriteStatus: async () => ({ running: true }),
  });

  expect(await screen.findByText("A merge action is in progress for this task. This status updates automatically.")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
});

it("does not re-enable merge actions on a polling tick while the write is still running", async () => {
  vi.useFakeTimers();
  try {
    const confirmMergeAbortAndStart = vi.fn().mockResolvedValue({ started: true });
    // The lock is genuinely still held for the whole of this test: a
    // polling tick must not override that with "the task is still
    // mergeConflict, so let the user click again".
    const getMergeConflictWriteStatus = vi.fn().mockResolvedValue({ running: true });
    renderActiveTask("mergeConflict", {
      getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
      getMergeConflictWriteStatus,
      confirmMergeAbortAndStart,
    });

    await vi.waitFor(() => expect(getMergeConflictWriteStatus).toHaveBeenCalled());
    await vi.advanceTimersByTimeAsync(2000);
    await vi.advanceTimersByTimeAsync(2000);

    expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
    expect(confirmMergeAbortAndStart).not.toHaveBeenCalled();
  } finally {
    vi.useRealTimers();
  }
});

it("restores the outcome's permitted actions only once the runtime confirms the write finished", async () => {
  vi.useFakeTimers();
  try {
    const getMergeConflictWriteStatus = vi
      .fn()
      .mockResolvedValueOnce({ running: true })
      .mockResolvedValue({ running: false });
    renderActiveTask("mergeConflict", {
      getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
      getMergeConflictWriteStatus,
    });

    await vi.waitFor(() =>
      expect(screen.queryByText("A merge action is in progress for this task. This status updates automatically.")).not.toBeNull(),
    );
    expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();

    await vi.advanceTimersByTimeAsync(2000);

    await vi.waitFor(() =>
      expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).not.toBeNull(),
    );
    expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).not.toBeNull();
  } finally {
    vi.useRealTimers();
  }
});

it("offers no merge action and shows content-free copy while the write status is loading or failed", async () => {
  const { unmount } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    getMergeConflictWriteStatus: () => new Promise(() => {}),
  });
  expect(await screen.findByText("Checking whether a merge action is currently running…")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
  unmount();

  // A malformed payload reaches this page as exactly this rejection: the
  // runtime guard in `src/ipc/merge_conflict_write_status.ts` refuses a
  // response carrying any extra field (covered in that module's own tests),
  // so the page never sees one and only has to fail safe on the rejection.
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    getMergeConflictWriteStatus: async () => {
      throw new FrontendError({
        code: "IPC_INVALID_RESPONSE",
        message: "PRIVATEPATH stdout leaked",
        severity: "error",
        retry: "never",
      });
    },
  });
  expect(
    await screen.findByText("The merge action status could not be checked safely. No merge action is offered until it can be."),
  ).toBeVisible();
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
  expect(document.body.textContent).not.toContain("PRIVATEPATH");
});

it("re-reads the write status after an abort reports started: false", async () => {
  const confirmMergeAbortAndStart = vi.fn().mockResolvedValue({ started: false });
  const getMergeConflictWriteStatus = vi.fn().mockResolvedValue({ running: false });
  const { isolation } = renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "confirmedUnresolved", counts: noConflictCounts }),
    getMergeConflictWriteStatus,
    confirmMergeAbortAndStart,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Abort the in-progress merge" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: "I approve aborting the in-progress merge and cancelling this task." }));
  const callsBefore = getMergeConflictWriteStatus.mock.calls.length;
  fireEvent.click(screen.getByRole("button", { name: "Confirm abort" }));

  await waitFor(() => expect(getMergeConflictWriteStatus.mock.calls.length).toBeGreaterThan(callsBefore));
  expect(getMergeConflictWriteStatus).toHaveBeenLastCalledWith(isolation.taskId);
});

it("swallows a merge-continue rejection into the fixed busy notice only when the runtime confirms a write is running", async () => {
  const confirmManualResolutionAndStartMergeContinue = vi.fn().mockRejectedValue(
    new FrontendError({
      code: "APP_CONFLICT",
      message: "PRIVATEPATH another merge-conflict write is in flight",
      severity: "error",
      retry: "afterStateRefresh",
    }),
  );
  const getMergeConflictWriteStatus = vi
    .fn()
    .mockResolvedValueOnce({ running: false })
    .mockResolvedValue({ running: true });
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    getMergeConflictWriteStatus,
    confirmManualResolutionAndStartMergeContinue,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Confirm the staged merge resolution" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /I reviewed the staged merge resolution/ }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm and continue" }));

  expect(await screen.findByText("A merge action is in progress for this task. This status updates automatically.")).toBeVisible();
  expect(screen.queryByRole("heading", { name: "Confirm the staged merge resolution" })).toBeNull();
  expect(document.body.textContent).not.toContain("PRIVATEPATH");
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
});

it("never offers the continue and abort actions as simultaneously executable once one is started", async () => {
  const confirmMergeAbortAndStart = vi.fn().mockResolvedValue({ started: true });
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "resolvedPendingConfirmation", counts: noConflictCounts }),
    getMergeConflictWriteStatus: async () => ({ running: false }),
    confirmMergeAbortAndStart,
  });

  // Both are offered while nothing is running; that is the only moment they
  // coexist, and choosing either must immediately withdraw both.
  expect(await screen.findByRole("button", { name: "Confirm the staged merge resolution" })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Abort the in-progress merge" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: "I approve aborting the in-progress merge and cancelling this task." }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm abort" }));

  await waitFor(() => expect(confirmMergeAbortAndStart).toHaveBeenCalledTimes(1));
  expect(await screen.findByText("A merge action is in progress for this task. This status updates automatically.")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Confirm the staged merge resolution" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Abort the in-progress merge" })).toBeNull();
});

it("shows a safe already-processing notice without a raw error when the abort command reports started: false", async () => {
  const confirmMergeAbortAndStart = vi.fn().mockResolvedValue({ started: false });
  renderActiveTask("mergeConflict", {
    getMergeConflictInspection: async () => ({ outcome: "confirmedUnresolved", counts: noConflictCounts }),
    confirmMergeAbortAndStart,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Abort the in-progress merge" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: "I approve aborting the in-progress merge and cancelling this task." }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm abort" }));

  expect(
    await screen.findByText("A merge action is already processing for this task, or its status needs to be refreshed. This status updates automatically."),
  ).toBeVisible();
  expect(screen.getByRole("heading", { name: "Abort the in-progress merge" })).toBeVisible();
  expect(confirmMergeAbortAndStart).toHaveBeenCalledTimes(1);
});
