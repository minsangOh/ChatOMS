import "../test/setup";
import { open } from "@tauri-apps/plugin-dialog";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { TaskDto } from "../ipc/types";
import { createFakeClient } from "../test/fixtures";
import { UserDiffReviewModal } from "./UserDiffReviewModal";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const mergingTask: TaskDto = {
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
};

it("starts merge through the combined approval command and prevents duplicate clicks", async () => {
  let resolveMerge: ((task: TaskDto) => void) | undefined;
  const approveUserDiffAndStartMerge = vi.fn(
    () => new Promise<TaskDto>((resolve) => { resolveMerge = resolve; }),
  );
  const onMergeStarted = vi.fn();
  render(
    <UserDiffReviewModal
      client={createFakeClient({
        getUserDiffForReview: async () => ({ diffText: "diff --git a/x b/x\n+line\n", diffContentHash: "a".repeat(64) }),
        getProjectRootValidationApprovalStatus: async () => ({ testApproved: true, buildApproved: true }),
        approveUserDiffAndStartMerge,
      })}
      taskId="task-id"
      taskVersion={3}
      onClose={vi.fn()}
      onMergeStarted={onMergeStarted}
    />,
  );

  const button = await screen.findByRole("button", { name: "Approve and start merge" });
  fireEvent.click(button);
  fireEvent.click(button);
  expect(approveUserDiffAndStartMerge).toHaveBeenCalledTimes(1);
  expect(button).toBeDisabled();
  resolveMerge?.(mergingTask);
  await waitFor(() => expect(onMergeStarted).toHaveBeenCalledWith(mergingTask));
});

it("keeps merge disabled until the separate ProjectRoot Test and Build approval succeeds", async () => {
  const approveUserDiffAndStartMerge = vi.fn();
  const approveProjectRootValidation = vi.fn().mockResolvedValue({ testApproved: true, buildApproved: true });
  render(
    <UserDiffReviewModal
      client={createFakeClient({
        getUserDiffForReview: async () => ({ diffText: "diff --git a/x b/x\n+line\n", diffContentHash: "a".repeat(64) }),
        getProjectRootValidationApprovalStatus: async () => ({ testApproved: false, buildApproved: false }),
        approveProjectRootValidation,
        approveUserDiffAndStartMerge,
      })}
      taskId="task-id"
      taskVersion={3}
      onClose={vi.fn()}
      onMergeStarted={vi.fn()}
    />,
  );

  const merge = await screen.findByRole("button", { name: "Approve and start merge" });
  expect(merge).toBeDisabled();
  fireEvent.click(merge);
  expect(approveUserDiffAndStartMerge).not.toHaveBeenCalled();

  vi.mocked(open).mockResolvedValueOnce("C:\\tools\\cargo\\bin\\cargo.exe");
  fireEvent.click(screen.getByLabelText("I approve post-merge Cargo Test and Build for this task version."));
  fireEvent.click(screen.getByRole("button", { name: "Select Cargo executable and approve" }));

  await waitFor(() => expect(approveProjectRootValidation).toHaveBeenCalledWith("task-id", 3, {
    executablePath: "C:\\tools\\cargo\\bin\\cargo.exe",
    cargoHomePath: null,
    rustupHomePath: null,
  }));
  await waitFor(() => expect(merge).toBeEnabled());
});

it("keeps merge disabled when ProjectRoot approval status cannot be read", async () => {
  const approveUserDiffAndStartMerge = vi.fn();
  render(
    <UserDiffReviewModal
      client={createFakeClient({
        getUserDiffForReview: async () => ({ diffText: "diff --git a/x b/x\n+line\n", diffContentHash: "a".repeat(64) }),
        getProjectRootValidationApprovalStatus: async () => { throw new Error("unavailable"); },
        approveUserDiffAndStartMerge,
      })}
      taskId="task-id"
      taskVersion={3}
      onClose={vi.fn()}
      onMergeStarted={vi.fn()}
    />,
  );

  expect(await screen.findByText("Post-merge validation approval could not be checked. Merge remains unavailable.")).toBeVisible();
  expect(screen.getByRole("button", { name: "Approve and start merge" })).toBeDisabled();
  expect(approveUserDiffAndStartMerge).not.toHaveBeenCalled();
});
