import { useEffect, useState } from "react";
import type { IpcClient } from "../ipc/client";
import { FrontendError, toFrontendError } from "../ipc/errors";

interface UserDiffReviewModalProps {
  client: IpcClient;
  taskId: string;
  taskVersion: number;
  onClose: () => void;
}

type DiffLoadState =
  | { kind: "loading" }
  | { kind: "loaded"; diffText: string; diffContentHash: string }
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
export function UserDiffReviewModal({ client, taskId, taskVersion, onClose }: UserDiffReviewModalProps) {
  const [load, setLoad] = useState<DiffLoadState>({ kind: "loading" });
  const [approving, setApproving] = useState(false);
  const [approved, setApproved] = useState(false);
  const [approvalError, setApprovalError] = useState<FrontendError | null>(null);

  useEffect(() => {
    let active = true;
    setLoad({ kind: "loading" });
    setApproving(false);
    setApproved(false);
    setApprovalError(null);
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
    return () => { active = false; };
  }, [client, taskId, taskVersion]);

  const confirm = async () => {
    if (load.kind !== "loaded") return;
    setApproving(true);
    setApprovalError(null);
    try {
      await client.approveUserDiff(taskId, taskVersion, load.diffContentHash);
      setApproved(true);
    } catch (error: unknown) {
      setApprovalError(toFrontendError(error));
    } finally {
      setApproving(false);
    }
  };

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
      {load.kind === "loaded" && !approved && <>
        <pre className="diff-text" aria-label="Diff content">{load.diffText}</pre>
        {approvalError && <div className="inline-notice" role="alert"><strong>{approvalError.message}</strong><span className="identifier">{approvalError.code}</span></div>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={onClose} disabled={approving}>Close</button>
          <button className="button" type="button" onClick={() => void confirm()} disabled={approving}>Approve this diff</button>
        </div>
      </>}
      {approved && <>
        <p className="muted">Diff approval recorded for the current task version.</p>
        <div className="form-actions">
          <button className="button" type="button" onClick={onClose}>Close</button>
        </div>
      </>}
    </section>
  </div>;
}
