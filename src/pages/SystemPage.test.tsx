import "../test/setup";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { FrontendError } from "../ipc/errors";
import type { SystemStatusDto } from "../ipc/types";
import {
  bootstrapStatus,
  createFakeClient,
  health,
  systemStatus,
  version,
} from "../test/fixtures";
import { SystemPage } from "./SystemPage";

it("shows loading before the core system request settles", () => {
  const client = createFakeClient({
    getSystemStatus: () => new Promise<SystemStatusDto>(() => undefined),
  });
  render(<SystemPage client={client} />);
  expect(screen.getByRole("status")).toHaveTextContent("Loading system status");
});

it("renders healthy system, version, readiness, no lease, and capabilities", async () => {
  render(<SystemPage client={createFakeClient()} />);
  expect(await screen.findByRole("heading", { level: 1, name: "System" })).toBeVisible();
  expect(screen.getByText("0.1.0")).toBeVisible();
  expect(screen.getByText("No active task currently holds the application lease.")).toBeVisible();
  expect(screen.getByText("Secure storage")).toBeVisible();
  expect(screen.getByText("Git execution")).toBeVisible();
  expect(screen.getAllByText("Healthy").length).toBeGreaterThan(0);
  expect(screen.getAllByText("Ready").length).toBeGreaterThanOrEqual(3);
  expect(screen.getAllByText("Supported").length).toBeGreaterThanOrEqual(2);
});

it("renders degraded, unavailable, and active task states without synthesis", async () => {
  const degraded: SystemStatusDto = {
    ...systemStatus,
    health: "degraded",
    loggingStatus: "unavailable",
    activeTaskStatus: { status: "active", taskId: "01900000-task", acquiredAtMs: 0 },
  };
  const { rerender } = render(
    <SystemPage
      client={createFakeClient({
        getHealth: async () => ({ status: "degraded" }),
        getSystemStatus: async () => degraded,
      })}
    />,
  );
  expect((await screen.findAllByText("Degraded")).length).toBeGreaterThanOrEqual(2);
  expect(screen.getByText("01900000-task")).toBeVisible();
  expect(screen.getAllByText("Unavailable").length).toBeGreaterThan(0);

  const unavailable = { ...systemStatus, health: "unavailable" as const };
  rerender(
    <SystemPage
      client={createFakeClient({
        getHealth: async () => ({ status: "unavailable" }),
        getSystemStatus: async () => unavailable,
      })}
    />,
  );
  expect((await screen.findAllByText("Unavailable")).length).toBeGreaterThan(0);
});

it("keeps the page usable when auxiliary calls fail", async () => {
  render(
    <SystemPage
      client={createFakeClient({
        getVersion: async () => {
          throw "raw auxiliary failure";
        },
      })}
    />,
  );
  expect(await screen.findByRole("heading", { level: 1, name: "System" })).toBeVisible();
  expect(screen.getByText("0.1.0")).toBeVisible();
  expect(screen.getByRole("status")).toHaveTextContent("IPC_REQUEST_FAILED");
});

it("renders safe core errors and retries the same five requests", async () => {
  const getSystemStatus = vi
    .fn()
    .mockRejectedValueOnce(
      new FrontendError({
        code: "APP_STORAGE_UNAVAILABLE",
        message: "Secure local storage is unavailable.",
        severity: "error",
        retry: "immediate",
      }),
    )
    .mockResolvedValue(systemStatus);
  const getVersion = vi.fn(async () => version);
  const getHealth = vi.fn(async () => health);
  const getBootstrapStatus = vi.fn(async () => bootstrapStatus);
  const getLegacyMigrationDiagnostic = vi.fn(async () => null);
  render(
    <SystemPage
      client={createFakeClient({
        getVersion,
        getHealth,
        getSystemStatus,
        getBootstrapStatus,
        getLegacyMigrationDiagnostic,
      })}
    />,
  );

  expect(await screen.findByText("Secure local storage is unavailable.")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("heading", { level: 1, name: "System" })).toBeVisible();
  await waitFor(() => expect(getSystemStatus).toHaveBeenCalledTimes(2));
  expect(getVersion).toHaveBeenCalledTimes(2);
  expect(getHealth).toHaveBeenCalledTimes(2);
  expect(getBootstrapStatus).toHaveBeenCalledTimes(2);
  expect(getLegacyMigrationDiagnostic).toHaveBeenCalledTimes(2);
});

it("renders only safe legacy migration identity diagnostics when startup is blocked", async () => {
  render(
    <SystemPage
      client={createFakeClient({
        getSystemStatus: async () => {
          throw new FrontendError({
            code: "APP_MIGRATION_FAILURE",
            message: "Local database migration failed.",
            severity: "critical",
            retry: "never",
          });
        },
        getLegacyMigrationDiagnostic: async () => ({
          projectId: "01900000-project",
          displayPath: "%USERPROFILE%\\repo",
          reasonCode: "stable filesystem identity was not confirmed",
        }),
      })}
    />,
  );
  expect(
    await screen.findByRole("heading", { name: "Legacy project verification stopped" }),
  ).toBeVisible();
  expect(screen.getByText("01900000-project")).toBeVisible();
  expect(screen.getByText("%USERPROFILE%\\repo")).toBeVisible();
  expect(document.body.textContent).not.toContain("C:\\private");
});

it("does not render extra source, path, SQL, SID, or secret fields", async () => {
  const error = Object.assign(
    new FrontendError({
      code: "APP_INTERNAL",
      message: "An internal error occurred.",
      severity: "critical",
      retry: "never",
    }),
    { source: "SELECT secret FROM C:\\private S-1-5-21" },
  );
  render(
    <SystemPage
      client={createFakeClient({
        getSystemStatus: async () => {
          throw error;
        },
      })}
    />,
  );
  expect(await screen.findByText("An internal error occurred.")).toBeVisible();
  expect(document.body.textContent).not.toMatch(/SELECT|C:\\private|S-1-5-21|secret/);
});
