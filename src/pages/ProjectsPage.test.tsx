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
    ["recoveryRequired", "requires manual recovery"],
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

it("does not call approveUserDiff before confirmation, and confirm sends only the digest, never raw diff text", async () => {
  const diffText = "diff --git a/x b/x\n+added line\n";
  const diffContentHash = "a".repeat(64);
  const getUserDiffForReview = vi.fn().mockResolvedValue({ diffText, diffContentHash });
  const approveUserDiff = vi.fn().mockResolvedValue({ approvedAtMs: 100 });
  const { isolation } = renderActiveTask("awaitingUserDiffApproval", {
    getReviewResult: async () => null,
    getUserDiffForReview,
    approveUserDiff,
  });

  fireEvent.click(await screen.findByRole("button", { name: "Review current diff" }));
  await screen.findByText(/\+added line/);
  expect(approveUserDiff).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole("button", { name: "Approve this diff" }));

  expect(await screen.findByText("Diff approval recorded for the current task version.")).toBeVisible();
  expect(approveUserDiff).toHaveBeenCalledWith(isolation.taskId, isolation.taskVersion, diffContentHash);
  expect(approveUserDiff).toHaveBeenCalledTimes(1);
  expect(approveUserDiff.mock.calls[0]).not.toContain(diffText);
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
