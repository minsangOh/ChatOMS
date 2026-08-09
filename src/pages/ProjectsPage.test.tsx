import "../test/setup";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { FrontendError } from "../ipc/errors";
import type { ProjectDto, TaskDto, TaskIsolationDto } from "../ipc/types";
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
