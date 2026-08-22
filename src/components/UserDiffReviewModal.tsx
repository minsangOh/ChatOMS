import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import type { IpcClient } from "../ipc/client";
import { FrontendError, toFrontendError } from "../ipc/errors";
import type { ProjectRootValidationApprovalStatusDto, TaskDto } from "../ipc/types";

interface UserDiffReviewModalProps {
  client: IpcClient;
  taskId: string;
  taskVersion: number;
  onClose: () => void;
  onMergeStarted: (task: TaskDto) => void;
}

type DiffLoadState =
  | { kind: "loading" }
  | { kind: "loaded"; diffText: string; diffContentHash: string }
  | { kind: "error" };

type ProjectRootApprovalLoadState =
  | { kind: "loading" }
  | { kind: "ready"; status: ProjectRootValidationApprovalStatusDto }
  | { kind: "error" };

/// Dedicated scoped review surface for the local-user-only raw diff
/// exception (see `docs/DECISIONS.md`). Fetches the current diff itself,
/// once, on mount, and keeps it only in this component's own local state --
/// never lifted to `ProjectsPage` state, never persisted, never sent
/// anywhere except back to `approveUserDiff` as a content-free digest.
/// Unmounting this component (its parent stops rendering it on close,
/// active task change, or task version change) discards all of that state
/// immediately; there is no cache, global store, or reusable hook that
/// could keep it alive past that point. Diff text is rendered as plain
/// text only -- no HTML injection, markdown rendering, syntax highlighting,
/// copy-to-clipboard, or download/export.
export function UserDiffReviewModal({ client, taskId, taskVersion, onClose, onMergeStarted }: UserDiffReviewModalProps) {
  const [load, setLoad] = useState<DiffLoadState>({ kind: "loading" });
  const [projectRootApproval, setProjectRootApproval] = useState<ProjectRootApprovalLoadState>({ kind: "loading" });
  const [projectRootApprovalConfirmed, setProjectRootApprovalConfirmed] = useState(false);
  const [approving, setApproving] = useState(false);
  const [approvingProjectRoot, setApprovingProjectRoot] = useState(false);
  const [approvalError, setApprovalError] = useState<FrontendError | null>(null);
  const [projectRootApprovalError, setProjectRootApprovalError] = useState<FrontendError | null>(null);

  useEffect(() => {
    let active = true;
    setLoad({ kind: "loading" });
    setProjectRootApproval({ kind: "loading" });
    setProjectRootApprovalConfirmed(false);
    setApproving(false);
    setApprovingProjectRoot(false);
    setApprovalError(null);
    setProjectRootApprovalError(null);
    void (async () => {
      try {
        const result = await client.getUserDiffForReview(taskId, taskVersion);
        if (active) {
          setLoad({ kind: "loaded", diffText: result.diffText, diffContentHash: result.diffContentHash });
        }
      } catch {
        if (active) setLoad({ kind: "error" });
      }
    })();
    void (async () => {
      try {
        const status = await client.getProjectRootValidationApprovalStatus(taskId, taskVersion);
        if (active) setProjectRootApproval({ kind: "ready", status });
      } catch {
        if (active) setProjectRootApproval({ kind: "error" });
      }
    })();
    return () => { active = false; };
  }, [client, taskId, taskVersion]);

  const confirm = async () => {
    if (load.kind !== "loaded") return;
    if (
      projectRootApproval.kind !== "ready" ||
      !projectRootApproval.status.testApproved ||
      !projectRootApproval.status.buildApproved
    ) return;
    setApproving(true);
    setApprovalError(null);
    try {
      const task = await client.approveUserDiffAndStartMerge(taskId, taskVersion, load.diffContentHash);
      onMergeStarted(task);
    } catch (error: unknown) {
      setApprovalError(toFrontendError(error));
    } finally {
      setApproving(false);
    }
  };

  const approveProjectRootValidation = async () => {
    if (projectRootApproval.kind !== "ready") return;
    if (projectRootApproval.status.testApproved && projectRootApproval.status.buildApproved) return;
    setApprovingProjectRoot(true);
    setProjectRootApprovalError(null);
    try {
      const executablePath = await open({
        directory: false,
        multiple: false,
        filters: [{ name: "Cargo executable", extensions: ["exe"] }],
      });
      if (typeof executablePath !== "string") return;
      const status = await client.approveProjectRootValidation(taskId, taskVersion, {
        executablePath,
        cargoHomePath: null,
        rustupHomePath: null,
      });
      setProjectRootApproval({ kind: "ready", status });
    } catch (error: unknown) {
      setProjectRootApprovalError(toFrontendError(error));
    } finally {
      setApprovingProjectRoot(false);
    }
  };

  const projectRootApprovalReady =
    projectRootApproval.kind === "ready" &&
    projectRootApproval.status.testApproved &&
    projectRootApproval.status.buildApproved;
  const busy = approving || approvingProjectRoot;

  return <div className="page-stack">
    <header className="page-header"><div><p className="eyebrow">User diff review</p><h1>Review current diff</h1></div></header>
    <section className="content-card" aria-labelledby="user-diff-review" aria-label="Review current diff">
      <h2 id="user-diff-review">Current diff</h2>
      {load.kind === "loading" && <p className="muted">Loading the current diff…</p>}
      {load.kind === "error" && <>
        <p className="inline-notice">The diff could not be loaded. Close and try again.</p>
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={onClose}>Close</button>
        </div>
      </>}
      {load.kind === "loaded" && <>
        <pre className="diff-text" aria-label="Diff content">{load.diffText}</pre>
        <section className="content-card" aria-labelledby="post-merge-validation-approval">
          <h2 id="post-merge-validation-approval">Approve post-merge Cargo validation</h2>
          <p>Approve these fixed commands separately before the merge starts. They will run only after a successful merge in a later step.</p>
          <ul>
            <li>Cargo Test</li>
            <li>Cargo Build</li>
          </ul>
          {projectRootApproval.kind === "loading" && <p className="muted">Checking post-merge validation approval…</p>}
          {projectRootApproval.kind === "error" && <p className="inline-notice">Post-merge validation approval could not be checked. Merge remains unavailable.</p>}
          {projectRootApproval.kind === "ready" && projectRootApprovalReady && <p className="muted">Post-merge Cargo Test and Build are approved for this task version.</p>}
          {projectRootApproval.kind === "ready" && !projectRootApprovalReady && <>
            <label className="checkbox-row">
              <input type="checkbox" checked={projectRootApprovalConfirmed} onChange={(event) => setProjectRootApprovalConfirmed(event.target.checked)} disabled={busy} />
              I approve post-merge Cargo Test and Build for this task version.
            </label>
            {projectRootApprovalError && <div className="inline-notice" role="alert"><strong>{projectRootApprovalError.message}</strong><span className="identifier">{projectRootApprovalError.code}</span></div>}
            <div className="form-actions">
              <button className="button button--secondary" type="button" onClick={() => void approveProjectRootValidation()} disabled={busy || !projectRootApprovalConfirmed}>Select Cargo executable and approve</button>
            </div>
          </>}
        </section>
        {approvalError && <div className="inline-notice" role="alert"><strong>{approvalError.message}</strong><span className="identifier">{approvalError.code}</span></div>}
        {!projectRootApprovalReady && <p className="inline-notice">Approve post-merge Cargo Test and Build before starting the merge.</p>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={onClose} disabled={busy}>Close</button>
          <button className="button" type="button" onClick={() => void confirm()} disabled={busy || !projectRootApprovalReady}>Approve and start merge</button>
        </div>
      </>}
    </section>
  </div>;
}
