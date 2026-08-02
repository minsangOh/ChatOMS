import "../test/setup";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { FrontendError } from "../ipc/errors";
import type { ProjectDto } from "../ipc/types";
import { createFakeClient } from "../test/fixtures";
import { ProjectsPage } from "./ProjectsPage";

const project: ProjectDto = {
  id: "01900000-0000-7000-8000-000000000001",
  name: "Foundation",
  createdAtMs: 1_700_000_000_000,
  updatedAtMs: 1_700_000_100_000,
};

it("renders loading and then the no-action empty state", async () => {
  render(<ProjectsPage client={createFakeClient()} />);
  expect(screen.getByRole("status")).toHaveTextContent("Loading projects");
  expect(await screen.findByRole("heading", { name: "No projects" })).toBeVisible();
  expect(screen.getByText("No projects have been registered yet.")).toBeVisible();
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
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
