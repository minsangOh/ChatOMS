import "../test/setup";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { FrontendError } from "../ipc/errors";
import { EmptyState } from "./EmptyState";
import { ErrorState } from "./ErrorState";
import { LoadingState } from "./LoadingState";
import { StatusBadge } from "./StatusBadge";

it("renders accessible loading, empty, and labeled status states", () => {
  const { rerender } = render(<LoadingState message="Loading projects…" />);
  expect(screen.getByRole("status")).toHaveTextContent("Loading projects");

  rerender(<EmptyState title="No projects" description="Nothing registered." />);
  expect(screen.getByRole("heading", { name: "No projects" })).toBeVisible();

  rerender(<StatusBadge status="notChecked" />);
  expect(screen.getByText("Not checked")).toBeVisible();
});

it("shows retry only for retryable dispositions", () => {
  const onRetry = vi.fn();
  const immediate = new FrontendError({
    code: "APP_TEMPORARY",
    message: "Try again.",
    severity: "error",
    retry: "immediate",
  });
  const { rerender } = render(<ErrorState error={immediate} onRetry={onRetry} />);
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(onRetry).toHaveBeenCalledOnce();

  const never = new FrontendError({
    code: "APP_INTERNAL",
    message: "An internal error occurred.",
    severity: "critical",
    retry: "never",
  });
  rerender(<ErrorState error={never} onRetry={onRetry} />);
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
});
