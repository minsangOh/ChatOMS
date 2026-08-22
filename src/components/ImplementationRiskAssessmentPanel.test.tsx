import "../test/setup";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { HIGH_RISK_CATEGORIES } from "../ipc/high_risk_approval";
import type { OperationRiskAssessmentStatusDto } from "../ipc/types";
import { ImplementationRiskAssessmentPanel } from "./ImplementationRiskAssessmentPanel";

function status(
  approved: readonly string[] = [],
  selectedCategories: OperationRiskAssessmentStatusDto["selectedCategories"] = [],
  declarationExists = false,
): OperationRiskAssessmentStatusDto {
  return {
    assessmentRequired: !declarationExists,
    declarationExists,
    selectedCategories,
    approvalReadiness: HIGH_RISK_CATEGORIES.map((riskCategory) => ({
      riskCategory,
      approved: approved.includes(riskCategory),
    })),
    failureCategory: null,
  };
}

it("requires a separate empty choice and confirmation before saving", async () => {
  const recorded = status([], [], true);
  const onDeclare = vi.fn().mockResolvedValue(recorded);
  const onRecorded = vi.fn();
  render(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "ready", status: status() }}
      busy={false}
      onDeclare={onDeclare}
      onRecorded={onRecorded}
    />,
  );

  const review = screen.getByRole("button", { name: "Review and confirm assessment" });
  expect(review).toBeDisabled();
  fireEvent.click(screen.getByLabelText("No high-risk categories apply to this implementation"));
  expect(review).toBeEnabled();
  fireEvent.click(review);
  expect(onDeclare).not.toHaveBeenCalled();
  const save = screen.getByRole("button", { name: "Record immutable assessment" });
  expect(save).toBeDisabled();
  fireEvent.click(screen.getByLabelText(/current task version and cannot be changed/i));
  fireEvent.click(save);
  await waitFor(() => expect(onDeclare).toHaveBeenCalledWith([], true));
  expect(onRecorded).toHaveBeenCalledWith(recorded);
});

it("requires confirmation and approval for every selected category", async () => {
  const onDeclare = vi.fn().mockResolvedValue(status(["dataMigration"], ["dataMigration"], true));
  render(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "ready", status: status(["dataMigration"]) }}
      busy={false}
      onDeclare={onDeclare}
      onRecorded={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByLabelText("Assess selected high-risk categories"));
  fireEvent.click(screen.getByLabelText("Database schema change"));
  expect(screen.getByRole("button", { name: "Review and confirm assessment" })).toBeDisabled();
  expect(screen.getByText("Approve every selected category before finalizing.")).toBeInTheDocument();
  fireEvent.click(screen.getByLabelText("Database schema change"));
  fireEvent.click(screen.getByLabelText("Data migration"));
  fireEvent.click(screen.getByRole("button", { name: "Review and confirm assessment" }));
  expect(onDeclare).not.toHaveBeenCalled();
  fireEvent.click(screen.getByLabelText(/current task version and cannot be changed/i));
  fireEvent.click(screen.getByRole("button", { name: "Record immutable assessment" }));
  await waitFor(() => expect(onDeclare).toHaveBeenCalledWith(["dataMigration"], false));
});

it("shows recorded empty and non-empty status without modification controls", () => {
  const { rerender } = render(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "ready", status: status([], [], true) }}
      busy={false}
      onDeclare={vi.fn()}
      onRecorded={vi.fn()}
    />,
  );
  expect(screen.getByText("No high-risk categories recorded")).toBeInTheDocument();
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  expect(screen.queryByRole("radio")).not.toBeInTheDocument();

  rerender(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "ready", status: status(["dataMigration"], ["dataMigration"], true) }}
      busy={false}
      onDeclare={vi.fn()}
      onRecorded={vi.fn()}
    />,
  );
  expect(screen.getByText("Data migration")).toBeInTheDocument();
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
});

it("fails safe for loading, malformed, stale, identity, and persistence failures", async () => {
  const { rerender } = render(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "loading" }}
      busy={false}
      onDeclare={vi.fn()}
      onRecorded={vi.fn()}
    />,
  );
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  rerender(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "error" }}
      busy={false}
      onDeclare={vi.fn()}
      onRecorded={vi.fn()}
    />,
  );
  expect(screen.getByText(/could not be loaded safely/i)).toBeInTheDocument();

  for (const failureCategory of ["versionConflict", "invalidState", "identityMismatch", "persistenceUnavailable"] as const) {
    rerender(
      <ImplementationRiskAssessmentPanel
        state={{
          kind: "ready",
          status: {
            assessmentRequired: null,
            declarationExists: null,
            selectedCategories: [],
            approvalReadiness: [],
            failureCategory,
          },
        }}
        busy={false}
        onDeclare={vi.fn()}
        onRecorded={vi.fn()}
      />,
    );
    expect(screen.getByText(/could not be loaded safely/i)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  }

  const onDeclare = vi.fn().mockRejectedValue(new Error("raw persistence error"));
  rerender(
    <ImplementationRiskAssessmentPanel
      state={{ kind: "ready", status: status() }}
      busy={false}
      onDeclare={onDeclare}
      onRecorded={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByLabelText("No high-risk categories apply to this implementation"));
  fireEvent.click(screen.getByRole("button", { name: "Review and confirm assessment" }));
  fireEvent.click(screen.getByLabelText(/current task version and cannot be changed/i));
  fireEvent.click(screen.getByRole("button", { name: "Record immutable assessment" }));
  await screen.findByText(/could not be recorded safely/i);
  expect(screen.queryByText("raw persistence error")).not.toBeInTheDocument();
});
