import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { EmptyState } from "../components/EmptyState";
import { ErrorState } from "../components/ErrorState";
import { ImplementationRiskAssessmentPanel } from "../components/ImplementationRiskAssessmentPanel";
import type { OperationRiskAssessmentLoadState } from "../components/ImplementationRiskAssessmentPanel";
import { LoadingState } from "../components/LoadingState";
import { UserDiffReviewModal } from "../components/UserDiffReviewModal";
import type { IpcClient } from "../ipc/client";
import { FrontendError, toFrontendError } from "../ipc/errors";
import { HIGH_RISK_CATEGORIES } from "../ipc/high_risk_approval";
import type { ApproveValidationCommandInput, EligibilityBlockingReason, HighRiskCategory, MergeConflictInspectionDto, PlanningResultDto, PostMergeValidationResultDto, ProjectCandidateDto, ProjectDto, ProjectStatusDto, ProviderEligibilityDto, ReviewResultDto, TaskBriefInput, TaskIsolationDto, ValidationCommandApprovalStatusDto, ValidationCommandCandidateDto, ValidationCommandKind } from "../ipc/types";

type ContextPackagePlanningReadinessLoadState =
  | { kind: "loading" }
  | { kind: "ready"; ready: boolean }
  | { kind: "error" };
type ContextPackageImplementationReadinessLoadState =
  | { kind: "loading" }
  | { kind: "ready"; ready: boolean }
  | { kind: "error" };
type ContextPackageReviewReadinessLoadState =
  | { kind: "loading" }
  | { kind: "ready"; ready: boolean }
  | { kind: "error" };
type HighRiskApprovalLoadState =
  | { kind: "loading" }
  | { kind: "ready"; approved: boolean }
  | { kind: "error" };

interface ProjectsPageProps { client: IpcClient; }
type ProjectsPageState = { kind: "loading" } | { kind: "error"; error: FrontendError } | { kind: "ready"; projects: ProjectDto[] };
interface TaskBriefForm { requirements: string; completionCriteria: string; prohibitedScope: string; }
type PlanningResultLoadState =
  | { kind: "loading" }
  | { kind: "ready"; result: PlanningResultDto | null }
  | { kind: "error" };
type ReviewResultLoadState =
  | { kind: "loading" }
  | { kind: "ready"; result: ReviewResultDto | null }
  | { kind: "error" };
type PostMergeValidationLoadState =
  | { kind: "loading" }
  | { kind: "ready"; results: readonly PostMergeValidationResultDto[] }
  | { kind: "error" };
/**
 * The authoritative answer to "is a merge-conflict Git write executing for
 * this task right now", as reported by the Tauri runtime's shared
 * `MergeConflictWriteLock`. `loading` and `error` are both treated as
 * fail-safe: no merge action is offered until a `ready` response says the
 * lock is free.
 */
type MergeConflictWriteStatusState =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; running: boolean };

type MergeConflictInspectionLoadState =
  | { kind: "loading" }
  | { kind: "ready"; result: MergeConflictInspectionDto | null }
  | { kind: "error" };
type ValidationCandidatesLoadState =
  | { kind: "loading" }
  | { kind: "ready"; candidates: readonly ValidationCommandCandidateDto[] }
  | { kind: "error" };
type ValidationApprovalLoadState =
  | { kind: "loading" }
  | { kind: "ready"; status: ValidationCommandApprovalStatusDto }
  | { kind: "error" };
interface ValidationCommandForm {
  executablePath: string;
  cargoHomePath: string;
  rustupHomePath: string;
  selectedKinds: readonly ValidationCommandKind[];
}
const emptyValidationCommandForm: ValidationCommandForm = {
  executablePath: "",
  cargoHomePath: "",
  rustupHomePath: "",
  selectedKinds: [],
};

export function ProjectsPage({ client }: ProjectsPageProps) {
  const [requestId, setRequestId] = useState(0);
  const [state, setState] = useState<ProjectsPageState>({ kind: "loading" });
  const [inputPath, setInputPath] = useState("");
  const [candidate, setCandidate] = useState<ProjectCandidateDto | null>(null);
  const [operationError, setOperationError] = useState<FrontendError | null>(null);
  const [busy, setBusy] = useState(false);
  const [statuses, setStatuses] = useState<Record<string, ProjectStatusDto>>({});
  const [isolations, setIsolations] = useState<Record<string, TaskIsolationDto>>({});
  const [activeTaskId, setActiveTaskId] = useState<string | null>(null);
  const [briefDialog, setBriefDialog] = useState<{ projectId: string } | null>(null);
  const [briefForm, setBriefForm] = useState<TaskBriefForm>({ requirements: "", completionCriteria: "", prohibitedScope: "" });
  const [briefError, setBriefError] = useState<string | null>(null);
  const [eligibilities, setEligibilities] = useState<Record<string, readonly ProviderEligibilityDto[]>>({});
  const [consentDialog, setConsentDialog] = useState<{ projectId: string; taskId: string; taskVersion: number; workKind: "planning" | "implementation" | "review" } | null>(null);
  const [contextPackagePrepDialog, setContextPackagePrepDialog] = useState<{ projectId: string; taskId: string; taskVersion: number; workKind: "planning" | "implementation" | "review" } | null>(null);
  const [contextPackagePreparationNotice, setContextPackagePreparationNotice] = useState<string | null>(null);
  const [contextPackagePlanningReadiness, setContextPackagePlanningReadiness] = useState<Record<string, ContextPackagePlanningReadinessLoadState>>({});
  const [contextPackageImplementationReadiness, setContextPackageImplementationReadiness] = useState<Record<string, ContextPackageImplementationReadinessLoadState>>({});
  const [contextPackageReviewReadiness, setContextPackageReviewReadiness] = useState<Record<string, ContextPackageReviewReadinessLoadState>>({});
  const [planningResults, setPlanningResults] = useState<Record<string, PlanningResultLoadState>>({});
  const [reviewResults, setReviewResults] = useState<Record<string, ReviewResultLoadState>>({});
  const [postMergeValidationResults, setPostMergeValidationResults] = useState<Record<string, PostMergeValidationLoadState>>({});
  const [mergeConflictInspections, setMergeConflictInspections] = useState<Record<string, MergeConflictInspectionLoadState>>({});
  const [mergeConflictWriteStatuses, setMergeConflictWriteStatuses] = useState<Record<string, MergeConflictWriteStatusState>>({});
  const [validationCandidates, setValidationCandidates] = useState<Record<string, ValidationCandidatesLoadState>>({});
  const [validationApprovals, setValidationApprovals] = useState<Record<string, ValidationApprovalLoadState>>({});
  const [validationForm, setValidationForm] = useState<ValidationCommandForm>(emptyValidationCommandForm);
  const [testingRuns, setTestingRuns] = useState<Record<string, boolean>>({});
  const [reviewRuns, setReviewRuns] = useState<Record<string, boolean>>({});
  const [highRiskApprovals, setHighRiskApprovals] = useState<Record<string, Partial<Record<HighRiskCategory, HighRiskApprovalLoadState>>>>({});
  const [operationRiskAssessments, setOperationRiskAssessments] = useState<Record<string, OperationRiskAssessmentLoadState>>({});
  const [highRiskApprovalDialog, setHighRiskApprovalDialog] = useState<{ projectId: string; taskId: string; taskVersion: number; category: HighRiskCategory } | null>(null);
  const [userDiffReviewDialog, setUserDiffReviewDialog] = useState<{ projectId: string; taskId: string; taskVersion: number } | null>(null);
  const [mergeContinueDialog, setMergeContinueDialog] = useState<{ projectId: string; taskId: string; taskVersion: number } | null>(null);
  const [mergeContinueConfirmed, setMergeContinueConfirmed] = useState(false);
  const [mergeAbortDialog, setMergeAbortDialog] = useState<{ projectId: string; taskId: string; taskVersion: number } | null>(null);
  const [mergeAbortConfirmed, setMergeAbortConfirmed] = useState(false);
  const [mergeAbortNotice, setMergeAbortNotice] = useState<string | null>(null);
  /**
   * Set the moment this page successfully asks the backend to start a
   * merge-conflict write, so a second click cannot get through before the
   * first status poll comes back. Unlike the flag it replaces, nothing
   * clears this on a timer: only an authoritative `running: false`, or the
   * task leaving `mergeConflict`, does.
   */
  const [mergeConflictWriteStarts, setMergeConflictWriteStarts] = useState<Record<string, boolean>>({});
  const [mergeConflictWriteNotices, setMergeConflictWriteNotices] = useState<Record<string, string>>({});

  useEffect(() => {
    let active = true;
    setState({ kind: "loading" });
    void (async () => {
      const projects = await client.listProjects();
      const activeTask = await client.getActiveTask();
      let restoredIsolation: TaskIsolationDto | null = null;
      if (activeTask !== null) {
        const task = await client.getTask(activeTask.taskId);
        const isolation = await client.getTaskIsolation(activeTask.taskId);
        if (task.projectId !== isolation.projectId || !projects.some((project) => project.id === task.projectId)) {
          throw new Error("Active task isolation data is inconsistent.");
        }
        restoredIsolation = isolation;
      }
      if (!active) return;
      setIsolations(restoredIsolation === null ? {} : { [restoredIsolation.projectId]: restoredIsolation });
      setActiveTaskId(activeTask?.taskId ?? null);
      setState({ kind: "ready", projects });
    })().catch((error: unknown) => {
      if (active) setState({ kind: "error", error: toFrontendError(error) });
    });
    return () => { active = false; };
  }, [client, requestId]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) =>
        (isolation.taskState === "worktreeReady" || isolation.taskState === "awaitingDesignApproval" || isolation.taskState === "reviewing") &&
        eligibilities[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        const result = await client.getProviderEligibility(isolation.taskId);
        if (active) setEligibilities((current) => ({ ...current, [isolation.taskId]: result }));
      }),
    ).catch(() => {});
    return () => { active = false; };
  }, [client, isolations, eligibilities]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) =>
        isolation.taskState === "worktreeReady" &&
        contextPackagePlanningReadiness[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setContextPackagePlanningReadiness((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const status = await client.getContextPackagePlanningReadiness(isolation.taskId, isolation.taskVersion);
          if (active) setContextPackagePlanningReadiness((current) => ({ ...current, [isolation.taskId]: { kind: "ready", ready: status.ready } }));
        } catch {
          if (active) setContextPackagePlanningReadiness((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, contextPackagePlanningReadiness]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) =>
        isolation.taskState === "awaitingDesignApproval" &&
        contextPackageImplementationReadiness[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setContextPackageImplementationReadiness((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const status = await client.getContextPackageImplementationReadiness(isolation.taskId, isolation.taskVersion);
          if (active) setContextPackageImplementationReadiness((current) => ({ ...current, [isolation.taskId]: { kind: "ready", ready: status.ready } }));
        } catch {
          if (active) setContextPackageImplementationReadiness((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, contextPackageImplementationReadiness]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) =>
        isolation.taskState === "reviewing" &&
        contextPackageReviewReadiness[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setContextPackageReviewReadiness((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const status = await client.getContextPackageReviewReadiness(isolation.taskId, isolation.taskVersion);
          if (active) setContextPackageReviewReadiness((current) => ({ ...current, [isolation.taskId]: { kind: "ready", ready: status.ready } }));
        } catch {
          if (active) setContextPackageReviewReadiness((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, contextPackageReviewReadiness]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) => isolation.taskState === "awaitingDesignApproval" && planningResults[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setPlanningResults((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const result = await client.getPlanningResult(isolation.taskId);
          if (active) setPlanningResults((current) => ({ ...current, [isolation.taskId]: { kind: "ready", result } }));
        } catch {
          if (active) setPlanningResults((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, planningResults]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) => isolation.taskState === "awaitingUserDiffApproval" && reviewResults[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setReviewResults((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const result = await client.getReviewResult(isolation.taskId);
          if (active) setReviewResults((current) => ({ ...current, [isolation.taskId]: { kind: "ready", result } }));
        } catch {
          if (active) setReviewResults((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, reviewResults]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) =>
        (isolation.taskState === "completed" || isolation.taskState === "recoveryRequired") &&
        postMergeValidationResults[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setPostMergeValidationResults((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const results = await client.getPostMergeValidationResults(isolation.taskId);
          if (active) setPostMergeValidationResults((current) => ({ ...current, [isolation.taskId]: { kind: "ready", results } }));
        } catch {
          if (active) setPostMergeValidationResults((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, postMergeValidationResults]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) => isolation.taskState === "mergeConflict" && mergeConflictInspections[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setMergeConflictInspections((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const result = await client.getMergeConflictInspection(isolation.taskId);
          if (active) setMergeConflictInspections((current) => ({ ...current, [isolation.taskId]: { kind: "ready", result } }));
        } catch {
          if (active) setMergeConflictInspections((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, mergeConflictInspections]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) => isolation.taskState === "mergeConflict" && mergeConflictWriteStatuses[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setMergeConflictWriteStatuses((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const status = await client.getMergeConflictWriteStatus(isolation.taskId);
          if (active) setMergeConflictWriteStatuses((current) => ({ ...current, [isolation.taskId]: { kind: "ready", running: status.running } }));
        } catch {
          if (active) setMergeConflictWriteStatuses((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, mergeConflictWriteStatuses]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) => isolation.taskState === "testing" && validationCandidates[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setValidationCandidates((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const candidates = await client.getValidationCommandCandidates(isolation.taskId);
          if (active) setValidationCandidates((current) => ({ ...current, [isolation.taskId]: { kind: "ready", candidates } }));
        } catch {
          if (active) setValidationCandidates((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, validationCandidates]);

  useEffect(() => {
    const pending = Object.values(isolations).filter(
      (isolation) => isolation.taskState === "testing" && validationApprovals[isolation.taskId] === undefined,
    );
    if (pending.length === 0) return;
    setValidationApprovals((current) => {
      const next = { ...current };
      for (const isolation of pending) next[isolation.taskId] = { kind: "loading" };
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (isolation) => {
        try {
          const status = await client.getValidationCommandApprovalStatus(isolation.taskId);
          if (active) setValidationApprovals((current) => ({ ...current, [isolation.taskId]: { kind: "ready", status } }));
        } catch {
          if (active) setValidationApprovals((current) => ({ ...current, [isolation.taskId]: { kind: "error" } }));
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, validationApprovals]);

  useEffect(() => {
    const pending: { taskId: string; taskVersion: number; category: HighRiskCategory }[] = [];
    for (const isolation of Object.values(isolations)) {
      if (isolation.taskState !== "awaitingDesignApproval") continue;
      const existing = highRiskApprovals[isolation.taskId];
      for (const category of HIGH_RISK_CATEGORIES) {
        if (existing?.[category] === undefined) {
          pending.push({ taskId: isolation.taskId, taskVersion: isolation.taskVersion, category });
        }
      }
    }
    if (pending.length === 0) return;
    setHighRiskApprovals((current) => {
      const next = { ...current };
      for (const item of pending) {
        next[item.taskId] = { ...next[item.taskId], [item.category]: { kind: "loading" } };
      }
      return next;
    });
    let active = true;
    void Promise.all(
      pending.map(async (item) => {
        try {
          const status = await client.getHighRiskApprovalStatus(item.taskId, item.taskVersion, item.category);
          if (active) {
            setHighRiskApprovals((current) => ({
              ...current,
              [item.taskId]: { ...current[item.taskId], [item.category]: { kind: "ready", approved: status.approved } },
            }));
          }
        } catch {
          if (active) {
            setHighRiskApprovals((current) => ({
              ...current,
              [item.taskId]: { ...current[item.taskId], [item.category]: { kind: "error" } },
            }));
          }
        }
      }),
    );
    return () => { active = false; };
  }, [client, isolations, highRiskApprovals]);

  useEffect(() => {
    const pending = Object.values(isolations).filter((isolation) =>
      isolation.taskState === "awaitingDesignApproval" &&
      operationRiskAssessments[operationRiskAssessmentKey(isolation.taskId, isolation.taskVersion)] === undefined,
    );
    if (pending.length === 0) return;
    setOperationRiskAssessments((current) => {
      const next = { ...current };
      for (const isolation of pending) {
        next[operationRiskAssessmentKey(isolation.taskId, isolation.taskVersion)] = { kind: "loading" };
      }
      return next;
    });
    let active = true;
    void Promise.all(pending.map(async (isolation) => {
      const key = operationRiskAssessmentKey(isolation.taskId, isolation.taskVersion);
      try {
        const status = await client.getProviderImplementationRiskAssessmentStatus(
          isolation.taskId,
          isolation.taskVersion,
        );
        if (active) {
          setOperationRiskAssessments((current) => ({ ...current, [key]: { kind: "ready", status } }));
        }
      } catch {
        if (active) {
          setOperationRiskAssessments((current) => ({ ...current, [key]: { kind: "error" } }));
        }
      }
    }));
    return () => { active = false; };
  }, [client, isolations, operationRiskAssessments]);

  useEffect(() => {
    setValidationForm(emptyValidationCommandForm);
  }, [activeTaskId]);

  useEffect(() => {
    setContextPackagePreparationNotice(null);
  }, [activeTaskId]);

  useEffect(() => {
    setUserDiffReviewDialog(null);
  }, [activeTaskId]);

  useEffect(() => {
    const activeExecutionEntries = Object.entries(isolations).filter(
      ([, isolation]) => isolation.taskState === "planning" || isolation.taskState === "implementing" || isolation.taskState === "testing" || isolation.taskState === "reviewing" || isolation.taskState === "merging" || isolation.taskState === "mergeConflict" || isolation.taskState === "postMergeTesting",
    );
    if (activeExecutionEntries.length === 0) return;
    const interval = setInterval(() => {
      void Promise.all(
        activeExecutionEntries.map(async ([projectId, isolation]) => {
          const next = await client.getTaskIsolation(isolation.taskId);
          setIsolations((current) => ({ ...current, [projectId]: next }));
          if (next.taskState === "mergeConflict") {
            try {
              const result = await client.getMergeConflictInspection(next.taskId);
              setMergeConflictInspections((current) => ({ ...current, [next.taskId]: { kind: "ready", result } }));
            } catch {
              setMergeConflictInspections((current) => ({ ...current, [next.taskId]: { kind: "error" } }));
            }
            // A background merge-conflict write never changes task state
            // until it is confirmed one way or another, so "still
            // `mergeConflict` after another tick" says nothing at all about
            // whether a write is running. The authoritative answer is the
            // runtime's shared `MergeConflictWriteLock`, so ask it — and
            // clear this page's local in-flight flag only when that lock
            // reports itself free, never on the strength of a tick.
            try {
              const status = await client.getMergeConflictWriteStatus(next.taskId);
              setMergeConflictWriteStatuses((current) => ({ ...current, [next.taskId]: { kind: "ready", running: status.running } }));
              if (!status.running) {
                setMergeConflictWriteStarts((current) => (current[next.taskId] ? { ...current, [next.taskId]: false } : current));
              }
            } catch {
              setMergeConflictWriteStatuses((current) => ({ ...current, [next.taskId]: { kind: "error" } }));
            }
          } else {
            // `cancelled`, `merging`, `postMergeTesting`, `recoveryRequired`
            // and friends: the merge-conflict surface is gone, so drop the
            // state that belongs to it rather than leaving a stale
            // in-flight flag behind for a later re-entry into
            // `mergeConflict`.
            clearMergeConflictWriteState(next.taskId);
          }
        }),
      ).catch(() => {});
    }, 2000);
    return () => clearInterval(interval);
  }, [client, isolations]);

  const clearMergeConflictWriteState = useCallback((taskId: string) => {
    setMergeConflictWriteStatuses((current) => { if (current[taskId] === undefined) return current; const next = { ...current }; delete next[taskId]; return next; });
    setMergeConflictWriteStarts((current) => { if (current[taskId] === undefined) return current; const next = { ...current }; delete next[taskId]; return next; });
    setMergeConflictWriteNotices((current) => { if (current[taskId] === undefined) return current; const next = { ...current }; delete next[taskId]; return next; });
    setMergeConflictInspections((current) => { if (current[taskId] === undefined) return current; const next = { ...current }; delete next[taskId]; return next; });
  }, []);
  const retry = useCallback(() => setRequestId((value) => value + 1), []);
  const run = async (operation: () => Promise<void>) => {
    setBusy(true);
    setOperationError(null);
    try { await operation(); } catch (error: unknown) { setOperationError(toFrontendError(error)); } finally { setBusy(false); }
  };
  const inspect = async (event?: FormEvent) => {
    event?.preventDefault();
    await run(async () => setCandidate(await client.inspectProjectCandidate(inputPath)));
  };
  const chooseFolder = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Choose a ChatOMS project" });
    if (typeof selected === "string") {
      setInputPath(selected);
      await run(async () => setCandidate(await client.inspectProjectCandidate(selected)));
    }
  };
  const register = async () => run(async () => {
    if (!candidate) return;
    await client.registerProject(inputPath, candidate.confirmationToken);
    setCandidate(null);
    setInputPath("");
    retry();
  });
  const validateBrief = (form: TaskBriefForm): string | null => {
    if (!form.requirements.trim()) return "Requirements are required.";
    if (!form.completionCriteria.trim()) return "Completion criteria are required.";
    if (!form.prohibitedScope.trim()) return "Prohibited scope is required.";
    return null;
  };
  const submitBrief = async (projectId: string) => {
    const error = validateBrief(briefForm);
    if (error) {
      setBriefError(error);
      return;
    }
    const brief: TaskBriefInput = {
      requirements: briefForm.requirements,
      completionCriteria: briefForm.completionCriteria,
      prohibitedScope: briefForm.prohibitedScope,
    };
    await run(async () => {
      setBriefDialog(null);
      const isolation = await client.createIsolationTask(projectId, brief);
      setIsolations((current) => ({ ...current, [projectId]: isolation }));
      setActiveTaskId(isolation.taskId);
      setBriefForm({ requirements: "", completionCriteria: "", prohibitedScope: "" });
      setBriefError(null);
    });
  };
  const approveInit = async (projectId: string, isolation: TaskIsolationDto) => run(async () => {
    const next = await client.approveGitInitialization(isolation.taskId, isolation.taskVersion);
    setIsolations((current) => ({ ...current, [projectId]: next }));
  });
  const createWorktree = async (projectId: string, isolation: TaskIsolationDto) => run(async () => {
    const next = await client.createTaskWorktree(isolation.taskId, isolation.taskVersion);
    setIsolations((current) => ({ ...current, [projectId]: next }));
  });
  const startWork = async () => {
    if (!consentDialog) return;
    const dialog = consentDialog;
    await run(async () => {
      const result = dialog.workKind === "planning"
        ? await client.startClaudePlanning(dialog.taskId, dialog.taskVersion)
        : dialog.workKind === "implementation"
        ? await client.startClaudeImplementation(dialog.taskId, dialog.taskVersion)
        : await client.startClaudeReview(dialog.taskId, dialog.taskVersion);
      setConsentDialog(null);
      setIsolations((current) => {
        const existing = current[dialog.projectId];
        if (!existing) return current;
        return { ...current, [dialog.projectId]: { ...existing, taskState: result.state, taskVersion: result.version } };
      });
      if (dialog.workKind === "review") {
        setReviewRuns((current) => ({ ...current, [dialog.taskId]: true }));
      }
    });
  };
  const prepareContextPackage = async () => {
    if (!contextPackagePrepDialog) return;
    const dialog = contextPackagePrepDialog;
    await run(async () => {
      if (dialog.workKind === "planning") {
        await client.preparePlanningContextPackage(dialog.taskId, dialog.taskVersion);
        // The cached readiness read for this task is now stale (it was
        // "not ready" before this call); clearing it makes the readiness
        // effect refetch and the activation button below reflect reality.
        setContextPackagePlanningReadiness((current) => {
          const next = { ...current };
          delete next[dialog.taskId];
          return next;
        });
      } else if (dialog.workKind === "implementation") {
        await client.prepareImplementationContextPackage(dialog.taskId, dialog.taskVersion);
        // Same reasoning as the planning branch above: this task's cached
        // Implementation readiness read was "not ready" before this call,
        // so clearing it makes the readiness effect refetch and the
        // activation button below reflect reality.
        setContextPackageImplementationReadiness((current) => {
          const next = { ...current };
          delete next[dialog.taskId];
          return next;
        });
      } else {
        await client.prepareReviewContextPackage(dialog.taskId, dialog.taskVersion);
        // Same reasoning as the planning/implementation branches above: this
        // task's cached Review readiness read was "not ready" before this
        // call, so clearing it makes the readiness effect refetch and the
        // activation button below reflect reality.
        setContextPackageReviewReadiness((current) => {
          const next = { ...current };
          delete next[dialog.taskId];
          return next;
        });
      }
      // Deliberately does not update `isolations` (unlike `startWork`, which
      // does): preparation never starts Claude and never changes this
      // task's state or version, so there is nothing here to refresh.
      setContextPackagePrepDialog(null);
      setContextPackagePreparationNotice(
        "Context Package v1 consent recorded. Claude was not started and this task's status is unchanged.",
      );
    });
  };
  const startContextPackagePlanning = async (projectId: string, taskId: string, taskVersion: number) => run(async () => {
    const result = await client.startClaudePlanningContextPackage(taskId, taskVersion);
    setIsolations((current) => {
      const existing = current[projectId];
      if (!existing) return current;
      return { ...current, [projectId]: { ...existing, taskState: result.state, taskVersion: result.version } };
    });
  });
  const startContextPackageImplementation = async (projectId: string, taskId: string, taskVersion: number) => run(async () => {
    const result = await client.startClaudeImplementationContextPackage(taskId, taskVersion);
    setIsolations((current) => {
      const existing = current[projectId];
      if (!existing) return current;
      return { ...current, [projectId]: { ...existing, taskState: result.state, taskVersion: result.version } };
    });
  });
  const startContextPackageReview = async (projectId: string, taskId: string, taskVersion: number) => run(async () => {
    const result = await client.startClaudeReviewContextPackage(taskId, taskVersion);
    setIsolations((current) => {
      const existing = current[projectId];
      if (!existing) return current;
      return { ...current, [projectId]: { ...existing, taskState: result.state, taskVersion: result.version } };
    });
    setReviewRuns((current) => ({ ...current, [taskId]: true }));
  });
  const cancelPlanning = async (taskId: string) => run(async () => {
    const result = await client.cancelClaudePlanning(taskId);
    if (!result.requested) {
      throw new FrontendError({
        code: "PLANNING_RUN_NOT_FOUND",
        message: "No active Claude Planning execution was found for this task. Refresh to check its current status.",
        severity: "error",
        retry: "afterStateRefresh",
      });
    }
  });
  const cancelImplementation = async (taskId: string) => run(async () => {
    const result = await client.cancelClaudeImplementation(taskId);
    if (!result.requested) {
      throw new FrontendError({
        code: "IMPLEMENTATION_RUN_NOT_FOUND",
        message: "No active Claude Implementation execution was found for this task. Refresh to check its current status.",
        severity: "error",
        retry: "afterStateRefresh",
      });
    }
  });
  const cancelReview = async (taskId: string) => run(async () => {
    const result = await client.cancelClaudeReview(taskId);
    if (!result.requested) {
      throw new FrontendError({
        code: "REVIEW_RUN_NOT_FOUND",
        message: "No active Claude Review execution was found for this task. Refresh to check its current status.",
        severity: "error",
        retry: "afterStateRefresh",
      });
    }
  });
  const toggleValidationCommandKind = (kind: ValidationCommandKind, checked: boolean) => {
    setValidationForm((current) => ({
      ...current,
      selectedKinds: checked
        ? [...current.selectedKinds, kind]
        : current.selectedKinds.filter((selected) => selected !== kind),
    }));
  };
  const approveValidationCommands = async (taskId: string, expectedVersion: number) => {
    const input: ApproveValidationCommandInput = {
      kinds: validationForm.selectedKinds,
      executablePath: validationForm.executablePath,
      cargoHomePath: validationForm.cargoHomePath.trim() === "" ? null : validationForm.cargoHomePath,
      rustupHomePath: validationForm.rustupHomePath.trim() === "" ? null : validationForm.rustupHomePath,
    };
    await run(async () => {
      try {
        const result = await client.approveValidationCommand(taskId, expectedVersion, input);
        setValidationApprovals((current) => ({ ...current, [taskId]: { kind: "ready", status: result } }));
        setValidationForm(emptyValidationCommandForm);
      } catch (error) {
        setValidationForm((current) => ({ ...current, executablePath: "", cargoHomePath: "", rustupHomePath: "" }));
        throw error;
      }
    });
  };
  const startValidationTesting = async (taskId: string, expectedVersion: number) => run(async () => {
    await client.startValidationTesting(taskId, expectedVersion);
    setTestingRuns((current) => ({ ...current, [taskId]: true }));
  });
  const cancelValidationTesting = async (taskId: string) => run(async () => {
    const result = await client.cancelValidationTesting(taskId);
    if (!result.requested) {
      throw new FrontendError({
        code: "TESTING_RUN_NOT_FOUND",
        message: "No active validation run was found for this task. Refresh to check its current status.",
        severity: "error",
        retry: "afterStateRefresh",
      });
    }
  });
  const confirmHighRiskApproval = async () => {
    if (!highRiskApprovalDialog) return;
    const dialog = highRiskApprovalDialog;
    await run(async () => {
      await client.approveHighRiskOperation(dialog.taskId, dialog.taskVersion, dialog.category);
      setHighRiskApprovalDialog(null);
      // Approve and reuse are not distinguished here: either way, only this
      // category's status for this task changes. Task state/version are
      // never touched by this call, so `isolations` is deliberately left
      // alone (mirrors `prepareContextPackage`'s reasoning).
      setHighRiskApprovals((current) => ({
        ...current,
        [dialog.taskId]: { ...current[dialog.taskId], [dialog.category]: { kind: "ready", approved: true } },
      }));
      const key = operationRiskAssessmentKey(dialog.taskId, dialog.taskVersion);
      setOperationRiskAssessments((current) => {
        const assessment = current[key];
        if (assessment?.kind !== "ready" || assessment.status.failureCategory !== null) return current;
        return {
          ...current,
          [key]: {
            kind: "ready",
            status: {
              ...assessment.status,
              approvalReadiness: assessment.status.approvalReadiness.map((entry) =>
                entry.riskCategory === dialog.category ? { ...entry, approved: true } : entry,
              ),
            },
          },
        };
      });
    });
  };
  const refreshMergeConflictWriteStatus = async (taskId: string): Promise<MergeConflictWriteStatusState> => {
    let next: MergeConflictWriteStatusState;
    try {
      const status = await client.getMergeConflictWriteStatus(taskId);
      next = { kind: "ready", running: status.running };
    } catch {
      next = { kind: "error" };
    }
    setMergeConflictWriteStatuses((current) => ({ ...current, [taskId]: next }));
    return next;
  };
  const confirmMergeContinue = async () => {
    if (!mergeContinueDialog) return;
    const dialog = mergeContinueDialog;
    await run(async () => {
      let result;
      try {
        result = await client.confirmManualResolutionAndStartMergeContinue(
          dialog.taskId,
          dialog.taskVersion,
        );
      } catch (error: unknown) {
        // The rejection may be the shared lock turning this call away
        // because a merge-conflict write is already running, or it may be a
        // genuine failure the user needs to read (a stale resolution
        // digest, say). The error code alone cannot tell those apart —
        // `APP_CONFLICT` covers both — so ask the authoritative lock
        // instead. Only a confirmed in-flight write is swallowed into the
        // fixed busy notice; everything else propagates to the existing
        // error surface with the dialog left open.
        const status = await refreshMergeConflictWriteStatus(dialog.taskId);
        if (status.kind === "ready" && status.running) {
          setMergeContinueDialog(null);
          setMergeContinueConfirmed(false);
          setMergeConflictWriteNotices((current) => ({ ...current, [dialog.taskId]: MERGE_CONFLICT_WRITE_BUSY_NOTICE }));
          return;
        }
        throw error;
      }
      setMergeContinueDialog(null);
      setMergeContinueConfirmed(false);
      // The write is now running and holds the shared lock. Nothing but an
      // authoritative `running: false` clears this.
      setMergeConflictWriteStarts((current) => ({ ...current, [dialog.taskId]: true }));
      setMergeConflictWriteNotices((current) => { const next = { ...current }; delete next[dialog.taskId]; return next; });
      setIsolations((current) => {
        const existing = current[dialog.projectId];
        if (!existing) return current;
        return { ...current, [dialog.projectId]: { ...existing, taskState: result.state, taskVersion: result.version } };
      });
    });
  };
  const confirmMergeAbort = async () => {
    if (!mergeAbortDialog) return;
    const dialog = mergeAbortDialog;
    await run(async () => {
      const result = await client.confirmMergeAbortAndStart(dialog.taskId, dialog.taskVersion);
      if (result.started) {
        setMergeAbortDialog(null);
        setMergeAbortConfirmed(false);
        setMergeAbortNotice(null);
        // Task state stays `mergeConflict` while the background abort runs.
        // This flag withholds both the continue and abort actions until the
        // shared `MergeConflictWriteLock` itself reports the write finished,
        // which is what keeps merge-continue and merge-abort from ever
        // appearing simultaneously executable for the same task.
        setMergeConflictWriteStarts((current) => ({ ...current, [dialog.taskId]: true }));
        setMergeConflictWriteNotices((current) => { const next = { ...current }; delete next[dialog.taskId]; return next; });
      } else {
        await refreshMergeConflictWriteStatus(dialog.taskId);
        setMergeAbortNotice(MERGE_CONFLICT_WRITE_BUSY_NOTICE);
        setMergeConflictWriteNotices((current) => ({ ...current, [dialog.taskId]: MERGE_CONFLICT_WRITE_BUSY_NOTICE }));
      }
    });
  };

  if (state.kind === "loading") return <LoadingState message="Loading projects" />;
  if (state.kind === "error") return <ErrorState error={state.error} onRetry={retry} />;

  if (briefDialog) {
    return <div className="page-stack">
      <header className="page-header"><div><p className="eyebrow">Create isolated task</p><h1>Enter task requirements</h1><p>Describe what the task should accomplish and what must be avoided.</p></div></header>
      <section className="content-card" aria-labelledby="brief-form"><h2 id="brief-form">Task brief</h2>
        <form onSubmit={(event) => { event.preventDefault(); void submitBrief(briefDialog.projectId); }}>
          <label htmlFor="requirements">Requirements</label>
          <textarea id="requirements" value={briefForm.requirements} onChange={(event) => setBriefForm((current) => ({ ...current, requirements: event.target.value }))} placeholder="What should the task accomplish?" disabled={busy} />
          <label htmlFor="completion-criteria">Completion criteria</label>
          <textarea id="completion-criteria" value={briefForm.completionCriteria} onChange={(event) => setBriefForm((current) => ({ ...current, completionCriteria: event.target.value }))} placeholder="How will you verify success?" disabled={busy} />
          <label htmlFor="prohibited-scope">Prohibited scope</label>
          <textarea id="prohibited-scope" value={briefForm.prohibitedScope} onChange={(event) => setBriefForm((current) => ({ ...current, prohibitedScope: event.target.value }))} placeholder="What must not be changed?" disabled={busy} />
          {briefError && <div className="inline-notice" role="alert"><strong>{briefError}</strong></div>}
          <div className="form-actions">
            <button className="button button--secondary" type="button" onClick={() => { setBriefDialog(null); setBriefError(null); }} disabled={busy}>Cancel</button>
            <button className="button" type="submit" disabled={busy}>Create task</button>
          </div>
        </form>
      </section>
    </div>;
  }

  if (consentDialog) {
    const consentCopy = consentDialogCopy(consentDialog.workKind);
    return <div className="page-stack">
      <header className="page-header"><div><p className="eyebrow">Provider consent</p><h1>{consentCopy.title}</h1><p>{consentCopy.description}</p></div></header>
      <section className="content-card" aria-labelledby="consent-form"><h2 id="consent-form">Provider transmission consent</h2>
        {operationError && <div className="inline-notice" role="alert"><strong>{operationError.message}</strong><span className="identifier">{operationError.code}</span></div>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={() => setConsentDialog(null)} disabled={busy}>Cancel</button>
          <button className="button" type="button" disabled={busy} onClick={() => void startWork()}>Confirm and start</button>
        </div>
      </section>
    </div>;
  }

  if (contextPackagePrepDialog) {
    const preparationCopy = contextPackagePreparationCopy(contextPackagePrepDialog.workKind);
    return <div className="page-stack">
      <header className="page-header"><div><p className="eyebrow">Context Package v1</p><h1>{preparationCopy.title}</h1><p>{preparationCopy.description}</p></div></header>
      <section className="content-card" aria-labelledby="context-package-prep-form"><h2 id="context-package-prep-form">Context Package v1 data-scope consent</h2>
        {operationError && <div className="inline-notice" role="alert"><strong>{operationError.message}</strong><span className="identifier">{operationError.code}</span></div>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={() => setContextPackagePrepDialog(null)} disabled={busy}>Cancel</button>
          <button className="button" type="button" disabled={busy} onClick={() => void prepareContextPackage()}>Confirm preparation</button>
        </div>
      </section>
    </div>;
  }

  if (userDiffReviewDialog) {
    const dialog = userDiffReviewDialog;
    return <UserDiffReviewModal
      client={client}
      taskId={dialog.taskId}
      taskVersion={dialog.taskVersion}
      onClose={() => setUserDiffReviewDialog(null)}
      onMergeStarted={(task) => {
        setUserDiffReviewDialog(null);
        setIsolations((current) => {
          const existing = current[dialog.projectId];
          if (!existing) return current;
          return { ...current, [dialog.projectId]: { ...existing, taskState: task.state, taskVersion: task.version } };
        });
      }}
    />;
  }

  if (highRiskApprovalDialog) {
    const categoryLabel = highRiskCategoryLabel(highRiskApprovalDialog.category);
    return <div className="page-stack">
      <header className="page-header"><div><p className="eyebrow">High-risk approval</p><h1>Approve {categoryLabel}</h1></div></header>
      <section className="content-card" aria-labelledby="high-risk-approval-form"><h2 id="high-risk-approval-form">{categoryLabel}</h2>
        <ul>
          <li>This approval applies only to the {categoryLabel} effect category for this task's current version.</li>
          <li>Approval does not run any provider and does not change this task's status.</li>
          <li>If the version changes, this approval cannot be reused.</li>
        </ul>
        {operationError && <div className="inline-notice" role="alert"><strong>{operationError.message}</strong><span className="identifier">{operationError.code}</span></div>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={() => setHighRiskApprovalDialog(null)} disabled={busy}>Cancel</button>
          <button className="button" type="button" disabled={busy} onClick={() => void confirmHighRiskApproval()}>Confirm approval</button>
        </div>
      </section>
    </div>;
  }

  if (mergeContinueDialog) {
    // Re-checked here, not only where the action was offered: if the shared
    // lock is taken (or its status becomes unreadable) while this dialog is
    // open, confirming must not be possible.
    const actionsAllowed = mergeConflictActionsAllowed(
      mergeConflictWriteStatuses[mergeContinueDialog.taskId],
      mergeConflictWriteStarts[mergeContinueDialog.taskId] === true,
    );
    return <div className="page-stack">
      <header className="page-header"><div><p className="eyebrow">Merge conflict resolution</p><h1>Confirm the staged merge resolution</h1><p>Git reports no unresolved entries. Continuing will create a merge commit from the currently staged resolution in the original checkout. ChatOMS will stop if that staged result changes before the commit.</p></div></header>
      <section className="content-card" aria-labelledby="merge-continue-confirm-form"><h2 id="merge-continue-confirm-form">Confirm and continue</h2>
        <label className="checkbox-row">
          <input type="checkbox" checked={mergeContinueConfirmed} onChange={(event) => setMergeContinueConfirmed(event.target.checked)} disabled={busy} />
          I reviewed the staged merge resolution and approve creating the merge commit.
        </label>
        <p className="muted">This confirmation is separate from the earlier task diff approval.</p>
        {!actionsAllowed && <p className="muted">{MERGE_CONFLICT_WRITE_BUSY_NOTICE}</p>}
        {operationError && <div className="inline-notice" role="alert"><strong>{operationError.message}</strong><span className="identifier">{operationError.code}</span></div>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={() => { setMergeContinueDialog(null); setMergeContinueConfirmed(false); }} disabled={busy}>Cancel</button>
          <button className="button" type="button" disabled={busy || !mergeContinueConfirmed || !actionsAllowed} onClick={() => void confirmMergeContinue()}>Confirm and continue</button>
        </div>
      </section>
    </div>;
  }

  if (mergeAbortDialog) {
    const actionsAllowed = mergeConflictActionsAllowed(
      mergeConflictWriteStatuses[mergeAbortDialog.taskId],
      mergeConflictWriteStarts[mergeAbortDialog.taskId] === true,
    );
    return <div className="page-stack">
      <header className="page-header"><div><p className="eyebrow">Merge conflict resolution</p><h1>Abort the in-progress merge</h1><p>This discards the staged merge resolution in the original checkout and restores it to the base commit it had before the merge started. Your task branch and its commit are not deleted. The task is then cancelled and cannot be resumed.</p></div></header>
      <section className="content-card" aria-labelledby="merge-abort-confirm-form"><h2 id="merge-abort-confirm-form">Confirm abort</h2>
        <label className="checkbox-row">
          <input type="checkbox" checked={mergeAbortConfirmed} onChange={(event) => setMergeAbortConfirmed(event.target.checked)} disabled={busy} />
          I approve aborting the in-progress merge and cancelling this task.
        </label>
        <p className="muted">This approval is separate from the earlier task diff approval and from any staged-resolution confirmation.</p>
        {mergeAbortNotice && <p className="muted">{mergeAbortNotice}</p>}
        {operationError && <div className="inline-notice" role="alert"><strong>{operationError.message}</strong><span className="identifier">{operationError.code}</span></div>}
        <div className="form-actions">
          <button className="button button--secondary" type="button" onClick={() => { setMergeAbortDialog(null); setMergeAbortConfirmed(false); setMergeAbortNotice(null); }} disabled={busy}>Cancel</button>
          <button className="button" type="button" disabled={busy || !mergeAbortConfirmed || !actionsAllowed} onClick={() => void confirmMergeAbort()}>Confirm abort</button>
        </div>
      </section>
    </div>;
  }

  return <div className="page-stack">
    <header className="page-header"><div><p className="eyebrow">Git isolation</p><h1>Projects</h1><p>Register a local project and create one isolated branch and worktree per task.</p></div><span className="count-label">{state.projects.length} total</span></header>

    <section className="content-card" aria-labelledby="register-project"><h2 id="register-project">Register project</h2>
      <form className="project-form" onSubmit={(event) => void inspect(event)}>
        <label htmlFor="project-path">Local directory</label>
        <div className="form-row"><input id="project-path" value={inputPath} onChange={(event) => { setInputPath(event.target.value); setCandidate(null); }} placeholder="C:\\path\\to\\project" disabled={busy} /><button className="button button--secondary" type="button" onClick={() => void chooseFolder()} disabled={busy}>Choose folder</button><button className="button" type="submit" disabled={busy || inputPath.trim() === ""}>Inspect</button></div>
      </form>
      {candidate && <div className="candidate-panel"><strong>{candidate.suggestedName}</strong><span className="identifier">{candidate.displayPath}</span><span>{candidate.repositoryKind === "git" ? "Existing Git repository" : "Non-Git directory"}</span><p>The displayed repository root will be registered. No files are changed during registration.</p><button className="button" onClick={() => void register()} disabled={busy}>Confirm registration</button></div>}
      {operationError && <div className="inline-notice" role="alert"><strong>{operationError.message}</strong><span className="identifier">{operationError.code}</span></div>}
    </section>

    {state.projects.length === 0 ? <EmptyState title="No projects" description="Choose or enter a local directory to register a project." /> :
      <ul className="project-list" aria-label="Projects">{state.projects.map((project) => {
        const isolation = isolations[project.id];
        const status = statuses[project.id];
        return <li className="project-card" key={project.id}><h2>{project.name}</h2><p className="identifier">{project.displayPath}</p><p className="identifier">{project.id}</p><dl className="detail-list detail-list--compact"><div><dt>Created</dt><dd>{formatTimestamp(project.createdAtMs)}</dd></div><div><dt>Updated</dt><dd>{formatTimestamp(project.updatedAtMs)}</dd></div></dl>
          <div className="card-actions"><button className="button button--secondary" disabled={busy} onClick={() => void run(async () => { const next = await client.getProjectGitStatus(project.id); setStatuses((current) => ({ ...current, [project.id]: next })); })}>Check Git status</button>{!isolation && activeTaskId === null && <button className="button" disabled={busy} onClick={() => { setBriefDialog({ projectId: project.id }); setBriefForm({ requirements: "", completionCriteria: "", prohibitedScope: "" }); setBriefError(null); }}>Create isolated task</button>}</div>
          {status && <p className="muted">{status.repositoryKind === "nonGit" ? "Git is not initialized." : status.repositoryStatus?.clean ? "Repository is clean." : "Repository is dirty; task isolation is blocked."}</p>}
          {isolation && <section className="isolation-panel" aria-label={`Isolation for ${project.name}`}><strong>{isolation.taskState}</strong><span className="identifier">{isolation.branchIdentity}</span>{isolation.blocker && <p className="inline-notice">{blockerMessage(isolation.blocker)}</p>}
            {isolation.taskState === "awaitingGitInitApproval" && <><p>Approval runs <code>git init</code>, stages files allowed by the existing .gitignore, and creates an initial snapshot commit. ChatOMS will not create or edit .gitignore or Git author settings.</p><button className="button" disabled={busy} onClick={() => void approveInit(project.id, isolation)}>Approve Git initialization</button></>}
            {(isolation.taskState === "projectValidated" || isolation.taskState === "gitInitialized") && <button className="button" disabled={busy} onClick={() => void createWorktree(project.id, isolation)}>Create managed worktree</button>}
            {isolation.taskState === "worktreeReady" && (() => {
              const entry = eligibilities[isolation.taskId]?.find((candidate) => candidate.workKind === "planning" && candidate.provider === "claude");
              const eligible = entry?.eligible ?? false;
              const readinessState = contextPackagePlanningReadiness[isolation.taskId];
              const contextPackageReady = readinessState?.kind === "ready" && readinessState.ready;
              return <div className="planning-panel">
                {entry && !eligible && <p className="inline-notice">{entry.blockingReasons.map(eligibilityBlockerMessage).join(" ")}</p>}
                <button className="button" disabled={busy || !eligible} onClick={() => setConsentDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "planning" })}>Start Claude Planning</button>
                <button className="button button--secondary" disabled={busy || !eligible} onClick={() => setContextPackagePrepDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "planning" })}>Prepare Context Package v1 consent</button>
                {contextPackagePreparationNotice && <p className="muted">{contextPackagePreparationNotice}</p>}
                <button className="button button--secondary" disabled={busy || !eligible || !contextPackageReady} onClick={() => void startContextPackagePlanning(project.id, isolation.taskId, isolation.taskVersion)}>Start Claude Planning (Context Package v1)</button>
                {readinessState?.kind === "ready" && !readinessState.ready && <p className="muted">Prepare Context Package v1 consent first.</p>}
                {readinessState?.kind === "error" && <p className="inline-notice">Context Package v1 readiness could not be loaded. Refresh to try again.</p>}
              </div>;
            })()}
            {isolation.taskState === "planning" && <div className="planning-panel">
              <p className="muted">Claude Planning is analyzing this task's requirements. This may take a few minutes.</p>
              <button className="button button--secondary" disabled={busy} onClick={() => void cancelPlanning(isolation.taskId)}>Cancel planning</button>
            </div>}
            {isolation.taskState === "awaitingDesignApproval" && <div className="planning-panel">
              <p className="muted">Claude Planning finished. The plan is awaiting design approval.</p>
              {renderPlanningResult(planningResults[isolation.taskId])}
              {(() => {
                const entry = eligibilities[isolation.taskId]?.find((candidate) => candidate.workKind === "implementation" && candidate.provider === "claude");
                const eligible = entry?.eligible ?? false;
                const readinessState = contextPackageImplementationReadiness[isolation.taskId];
                const contextPackageReady = readinessState?.kind === "ready" && readinessState.ready;
                return <div className="planning-panel">
                  {entry && !eligible && <p className="inline-notice">{entry.blockingReasons.map(eligibilityBlockerMessage).join(" ")}</p>}
                  <button className="button" disabled={busy || !eligible} onClick={() => setConsentDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "implementation" })}>Start Claude Implementation</button>
                  <button className="button button--secondary" disabled={busy || !eligible} onClick={() => setContextPackagePrepDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "implementation" })}>Prepare Context Package v1 consent</button>
                  {contextPackagePreparationNotice && <p className="muted">{contextPackagePreparationNotice}</p>}
                  <button className="button button--secondary" disabled={busy || !eligible || !contextPackageReady} onClick={() => void startContextPackageImplementation(project.id, isolation.taskId, isolation.taskVersion)}>Start Claude Implementation (Context Package v1)</button>
                  {readinessState?.kind === "ready" && !readinessState.ready && <p className="muted">Prepare Context Package v1 consent first.</p>}
                  {readinessState?.kind === "error" && <p className="inline-notice">Context Package v1 readiness could not be loaded. Refresh to try again.</p>}
                </div>;
              })()}
              <section className="high-risk-approval-panel" aria-label="High-risk approval">
                <h3>High-risk approval</h3>
                <ul>{HIGH_RISK_CATEGORIES.map((category) => {
                  const approvalState = highRiskApprovals[isolation.taskId]?.[category];
                  const approved = approvalState?.kind === "ready" && approvalState.approved;
                  return <li key={category} className="high-risk-approval-row">
                    <span>{highRiskCategoryLabel(category)}</span>
                    {approvalState === undefined || approvalState.kind === "loading" ? <span className="muted">Loading…</span>
                      : approvalState.kind === "error" ? <span className="inline-notice">Status could not be loaded.</span>
                      : approved ? <span className="muted">Approved</span>
                      : <button className="button button--secondary" disabled={busy} onClick={() => setHighRiskApprovalDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, category })}>Approve</button>}
                  </li>;
                })}</ul>
              </section>
              <ImplementationRiskAssessmentPanel
                state={operationRiskAssessments[operationRiskAssessmentKey(isolation.taskId, isolation.taskVersion)] ?? { kind: "loading" }}
                busy={busy}
                onDeclare={(categories, explicitEmpty) => client.declareProviderImplementationRisk(
                  isolation.taskId,
                  isolation.taskVersion,
                  categories,
                  explicitEmpty,
                )}
                onRecorded={(assessment) => setOperationRiskAssessments((current) => ({
                  ...current,
                  [operationRiskAssessmentKey(isolation.taskId, isolation.taskVersion)]: {
                    kind: "ready",
                    status: assessment,
                  },
                }))}
              />
            </div>}
            {isolation.taskState === "implementing" && <div className="planning-panel">
              <p className="muted">Claude Implementation is applying changes inside this task's isolated worktree. This may take a few minutes.</p>
              <button className="button button--secondary" disabled={busy} onClick={() => void cancelImplementation(isolation.taskId)}>Cancel implementation</button>
            </div>}
            {isolation.taskState === "paused" && <p className="muted">The task is paused. Resuming is not yet available in this build.</p>}
            {isolation.taskState === "testing" && (() => {
              const candidateState = validationCandidates[isolation.taskId];
              const approvalState = validationApprovals[isolation.taskId];
              const approvedKinds = approvalState?.kind === "ready" ? approvalState.status.approvedKinds : [];
              const candidates = candidateState?.kind === "ready" ? candidateState.candidates : [];
              const hasApproved = approvedKinds.length > 0;
              const running = testingRuns[isolation.taskId] === true;
              return <div className="testing-panel" aria-label="Testing validation">
                {running ? <>
                  <p className="muted">Validation is running inside this task's isolated worktree. This may take a few minutes.</p>
                  <button className="button button--secondary" disabled={busy} onClick={() => void cancelValidationTesting(isolation.taskId)}>Cancel validation</button>
                </> : <>
                  {(candidateState === undefined || candidateState.kind === "loading" || approvalState === undefined || approvalState.kind === "loading") && <p className="muted">Loading validation commands…</p>}
                  {candidateState?.kind === "error" && <p className="inline-notice">Validation commands could not be loaded. Refresh to try again.</p>}
                  {approvalState?.kind === "error" && <p className="inline-notice">Approval status could not be loaded. Refresh to try again.</p>}
                  {candidateState?.kind === "ready" && approvalState?.kind === "ready" && (
                    candidates.length === 0
                      ? <p className="muted">No Cargo validation commands were found for this task.</p>
                      : <form onSubmit={(event) => { event.preventDefault(); void approveValidationCommands(isolation.taskId, isolation.taskVersion); }}>
                          <label htmlFor="cargo-executable-path">Cargo executable path</label>
                          <input id="cargo-executable-path" value={validationForm.executablePath} onChange={(event) => setValidationForm((current) => ({ ...current, executablePath: event.target.value }))} placeholder="C:\tools\cargo\bin\cargo.exe" disabled={busy} />
                          <label htmlFor="cargo-home-path">CARGO_HOME (optional)</label>
                          <input id="cargo-home-path" value={validationForm.cargoHomePath} onChange={(event) => setValidationForm((current) => ({ ...current, cargoHomePath: event.target.value }))} disabled={busy} />
                          <label htmlFor="rustup-home-path">RUSTUP_HOME (optional)</label>
                          <input id="rustup-home-path" value={validationForm.rustupHomePath} onChange={(event) => setValidationForm((current) => ({ ...current, rustupHomePath: event.target.value }))} disabled={busy} />
                          <fieldset>
                            <legend>Validation commands</legend>
                            {candidates.map((candidate) => {
                              const isApproved = approvedKinds.includes(candidate.kind);
                              return <label key={candidate.kind} className="checkbox-row">
                                <input type="checkbox" checked={isApproved || validationForm.selectedKinds.includes(candidate.kind)} disabled={busy || isApproved} onChange={(event) => toggleValidationCommandKind(candidate.kind, event.target.checked)} />
                                {candidate.label}{isApproved ? " (approved)" : ""}
                              </label>;
                            })}
                          </fieldset>
                          <button className="button" type="submit" disabled={busy || validationForm.selectedKinds.length === 0 || validationForm.executablePath.trim() === ""}>Approve selected validation commands</button>
                        </form>
                  )}
                  <button className="button" disabled={busy || !hasApproved} onClick={() => void startValidationTesting(isolation.taskId, isolation.taskVersion)}>Start approved validation</button>
                  {!hasApproved && <p className="muted">Approve at least one validation command before starting.</p>}
                </>}
              </div>;
            })()}
            {isolation.taskState === "reviewing" && (() => {
              const entry = eligibilities[isolation.taskId]?.find((candidate) => candidate.workKind === "review" && candidate.provider === "claude");
              const eligible = entry?.eligible ?? false;
              const running = reviewRuns[isolation.taskId] === true;
              const readinessState = contextPackageReviewReadiness[isolation.taskId];
              const contextPackageReady = readinessState?.kind === "ready" && readinessState.ready;
              return <div className="review-panel" aria-label="Claude Review">
                {running ? <>
                  <p className="muted">Claude Review is analyzing the changes in this task's isolated worktree. This may take a few minutes.</p>
                  <button className="button button--secondary" disabled={busy} onClick={() => void cancelReview(isolation.taskId)}>Cancel review</button>
                </> : <>
                  {entry && !eligible && <p className="inline-notice">{entry.blockingReasons.map(eligibilityBlockerMessage).join(" ")}</p>}
                  <button className="button" disabled={busy || !eligible} onClick={() => setConsentDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "review" })}>Start Claude Review</button>
                  <button className="button button--secondary" disabled={busy || !eligible} onClick={() => setContextPackagePrepDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "review" })}>Prepare Context Package v1 consent</button>
                  {contextPackagePreparationNotice && <p className="muted">{contextPackagePreparationNotice}</p>}
                  <button className="button button--secondary" disabled={busy || !eligible || !contextPackageReady} onClick={() => void startContextPackageReview(project.id, isolation.taskId, isolation.taskVersion)}>Start Claude Review (Context Package v1)</button>
                  {readinessState?.kind === "ready" && !readinessState.ready && <p className="muted">Prepare Context Package v1 consent first.</p>}
                  {readinessState?.kind === "error" && <p className="inline-notice">Context Package v1 readiness could not be loaded. Refresh to try again.</p>}
                </>}
              </div>;
            })()}
            {isolation.taskState === "awaitingUserDiffApproval" && <div className="review-panel" aria-label="Claude Review result">
              <p className="muted">Claude Review finished. The review is awaiting your decision on the diff.</p>
              {renderReviewResult(reviewResults[isolation.taskId])}
              <button className="button button--secondary" disabled={busy} onClick={() => setUserDiffReviewDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion })}>Review current diff</button>
            </div>}
            {isolation.taskState === "merging" && <p className="muted">The approved change is being committed and merged. This status updates automatically; merge cancellation is not available.</p>}
            {isolation.taskState === "postMergeTesting" && <>
              <p className="muted">The merge completed. Post-merge validation is pending.</p>
              <p className="muted">Execution status updates automatically while validation runs.</p>
            </>}
            {isolation.taskState === "mergeConflict" && renderMergeConflictInspection(
              mergeConflictInspections[isolation.taskId],
              () => { setMergeAbortDialog(null); setMergeContinueDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion }); },
              () => { setMergeContinueDialog(null); setMergeAbortNotice(null); setMergeAbortDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion }); },
              mergeConflictWriteStatuses[isolation.taskId],
              mergeConflictWriteStarts[isolation.taskId] === true,
              mergeConflictWriteNotices[isolation.taskId],
            )}
            {isolation.taskState === "completed" && <>
              <p className="muted">This task is completed. Its active task lease has been released.</p>
              {renderPostMergeValidationResults(postMergeValidationResults[isolation.taskId])}
            </>}
            {isolation.taskState === "recoveryRequired" && <>
              <p className="muted">The task result could not be confirmed. Review the repository safely before proceeding.</p>
              {renderPostMergeValidationResults(postMergeValidationResults[isolation.taskId])}
            </>}
            {isolation.taskState === "failed" && <p className="inline-notice">Claude Planning failed. Review the task before retrying.</p>}
            {isolation.taskState === "cancelled" && <p className="muted">Claude Planning was cancelled.</p>}
            {isolation.taskState !== "mergeConflict" && <button className="button button--secondary" disabled={busy} onClick={() => void run(async () => { const next = await client.getTaskIsolation(isolation.taskId); setIsolations((current) => ({ ...current, [project.id]: next })); })}>Refresh isolation</button>}
          </section>}
        </li>;
      })}</ul>}
  </div>;
}

function consentDialogCopy(workKind: "planning" | "implementation" | "review"): { title: string; description: string } {
  switch (workKind) {
    case "implementation":
      return {
        title: "Send task brief and plan to Claude",
        description: "Claude Implementation will read this task's requirements, completion criteria, prohibited scope, and the approved plan, then create, edit, and delete files inside the isolated task worktree. It cannot run Bash or other shell commands.",
      };
    case "review":
      return {
        title: "Send task brief and diff to Claude",
        description: "Claude Review will read this task's requirements, completion criteria, prohibited scope, and the current Git diff of changes inside the isolated task worktree. It runs read-only and cannot create, edit, or delete files.",
      };
    case "planning":
    default:
      return {
        title: "Send task brief to Claude",
        description: "Claude Planning will read this task's requirements, completion criteria, and prohibited scope from a read-only copy of the worktree. It runs read-only and cannot create, edit, or delete files.",
      };
  }
}

function contextPackagePreparationCopy(workKind: "planning" | "implementation" | "review"): { title: string; description: string } {
  const categories = workKind === "implementation"
    ? "requirements, completion criteria, prohibited scope, and the approved plan"
    : workKind === "review"
    ? "requirements, completion criteria, prohibited scope, and the current Git diff"
    : "requirements, completion criteria, and prohibited scope";
  return {
    title: "Prepare Context Package v1 consent",
    description: `This records a one-time transmission consent and a content-free reference for the Context Package v1 data scope, covering ${categories}. Actual values are never shown here. This does not start Claude and does not change this task's status.`,
  };
}

function operationRiskAssessmentKey(taskId: string, taskVersion: number): string {
  return `${taskId}:${taskVersion}`;
}

function highRiskCategoryLabel(category: HighRiskCategory): string {
  switch (category) {
    case "architectureChange": return "Architecture change";
    case "databaseSchemaChange": return "Database schema change";
    case "authenticationOrAuthorizationChange": return "Authentication or authorization change";
    case "securityPolicyChange": return "Security policy change";
    case "externalNetworkBehaviorAddition": return "External network behavior addition";
    case "externalDataTransmissionAddition": return "External data transmission addition";
    case "largeScaleFileMoveOrDeletion": return "Large-scale file move or deletion";
    case "publicApiOrStorageFormatChange": return "Public API or storage format change";
    case "operatingSystemConfigurationChange": return "Operating system configuration change";
    case "administratorPrivilegesRequired": return "Administrator privileges required";
    case "breakingCompatibilityChange": return "Breaking compatibility change";
    case "dataMigration": return "Data migration";
    case "difficultToRecoverChange": return "Difficult-to-recover change";
    default: return category;
  }
}

function blockerMessage(blocker: TaskIsolationDto["blocker"]): string {
  switch (blocker) {
    case "dirtyRepository": return "Commit, stash, or remove tracked and untracked changes before creating isolation.";
    case "detachedHead": return "Check out a valid branch before creating isolation.";
    case "unbornRepository": return "Create at least one commit before creating isolation.";
    case "missingCurrentBranch": return "Check out a valid current branch before creating isolation.";
    case "gitAuthorMissing": return "Configure Git user.name and user.email. This task remains in RecoveryRequired until an explicit recovery flow is available.";
    case "recoveryRequired": return "Git effects could not be verified. Inspect the repository and managed path before continuing.";
    case "gitOperationFailed": return "The Git operation failed. ChatOMS did not delete or retry any branch or worktree; review the recovery state.";
    default: return "";
  }
}

function eligibilityBlockerMessage(reason: EligibilityBlockingReason): string {
  switch (reason) {
    case "capabilityUnavailable": return "Claude capability has not been checked yet.";
    case "capabilityUnsupported": return "Claude Code CLI is not available or not logged in.";
    case "contractNotApproved": return "This work is not approved for this provider yet.";
    case "taskStateMismatch": return "The task is not in a state that allows starting Claude Planning.";
    default: return "";
  }
}

function renderPlanningResult(state: PlanningResultLoadState | undefined) {
  if (state === undefined || state.kind === "loading") {
    return <p className="muted">Loading the plan…</p>;
  }
  if (state.kind === "error") {
    return <p className="inline-notice">The plan could not be loaded. Refresh to try again.</p>;
  }
  if (state.result === null || state.result.planText === null) {
    return <p className="muted">No plan is available for this task.</p>;
  }
  return <div className="plan-text-panel" aria-label="Claude Planning result">
    <pre className="plan-text">{state.result.planText}</pre>
  </div>;
}

function renderReviewResult(state: ReviewResultLoadState | undefined) {
  if (state === undefined || state.kind === "loading") {
    return <p className="muted">Loading the review…</p>;
  }
  if (state.kind === "error") {
    return <p className="inline-notice">The review could not be loaded. Refresh to try again.</p>;
  }
  if (state.result === null || state.result.reviewText === null) {
    return <p className="muted">No review is available for this task.</p>;
  }
  return <div className="review-text-panel" aria-label="Claude Review result">
    <pre className="review-text">{state.result.reviewText}</pre>
  </div>;
}

function renderPostMergeValidationResults(state: PostMergeValidationLoadState | undefined) {
  if (state === undefined || state.kind === "loading") {
    return <p className="muted">Loading post-merge validation results…</p>;
  }
  if (state.kind === "error") {
    return <p className="inline-notice">Post-merge validation results could not be loaded. Refresh to try again.</p>;
  }
  if (state.results.length === 0) {
    return <p className="muted">No post-merge validation results are available for this task.</p>;
  }
  return <section className="post-merge-validation-panel" aria-label="Post-merge validation results">
    <h3>Post-merge validation results</h3>
    <ul>
      {state.results.map((result) => <li key={`${result.commandKind}-${result.attemptSequence}`}>
        <strong>{result.commandKind === "test" ? "Test" : "Build"}</strong>
        <span className="muted">Outcome: {result.outcome}</span>
        <span>{result.safeSummary}</span>
        {result.exitCode !== null && <span className="muted">Exit code: {result.exitCode}</span>}
      </li>)}
    </ul>
  </section>;
}

const MERGE_CONFLICT_KIND_LABELS = [
  ["bothModified", "Both modified"],
  ["bothAdded", "Both added"],
  ["bothDeleted", "Both deleted"],
  ["addedByUs", "Added by us"],
  ["addedByThem", "Added by them"],
  ["deletedByUs", "Deleted by us"],
  ["deletedByThem", "Deleted by them"],
] as const;

function renderMergeConflictInspection(
  state: MergeConflictInspectionLoadState | undefined,
  onConfirm: () => void,
  onAbort: () => void,
  writeStatus: MergeConflictWriteStatusState | undefined,
  writeStartedLocally: boolean,
  writeNotice: string | undefined,
) {
  // The write-status gate is evaluated before the inspection outcome: an
  // action-eligible outcome means nothing while a merge-conflict write is
  // executing, and `loading`/`error`/absent are all fail-safe here. Only a
  // confirmed `running: false` from the runtime's shared lock, with no
  // locally started write outstanding, lets any action through.
  if (writeStatus === undefined || writeStatus.kind === "loading") {
    return <p className="muted">Checking whether a merge action is currently running…</p>;
  }
  if (writeStatus.kind === "error") {
    return <p className="inline-notice">The merge action status could not be checked safely. No merge action is offered until it can be.</p>;
  }
  if (!mergeConflictActionsAllowed(writeStatus, writeStartedLocally)) {
    return <div className="merge-conflict-panel">
      <p className="muted">A merge action is in progress for this task. This status updates automatically.</p>
      {writeNotice !== undefined && <p className="muted">{writeNotice}</p>}
    </div>;
  }
  if (state === undefined || state.kind === "loading") {
    return <p className="muted">Checking the Git merge state safely…</p>;
  }
  if (state.kind === "error") {
    return <p className="inline-notice">The merge conflict state could not be loaded safely.</p>;
  }
  if (state.result === null) {
    return <p className="muted">No merge conflict inspection is available.</p>;
  }
  // Only these three outcomes offer an abort action at all (see
  // `docs/PHASE_PLAN.md` Phase 5e-4); `inconsistent`/`unavailable` never do.
  const notice = writeNotice !== undefined ? <p className="muted">{writeNotice}</p> : null;
  const abortAction = <button className="button button--secondary" onClick={onAbort}>Abort the in-progress merge</button>;
  switch (state.result.outcome) {
    case "confirmedUnresolved":
      {
        const { counts } = state.result;
      return <div className="merge-conflict-panel">
        <div className="inline-notice">
          <p>Git reported merge conflicts. ChatOMS did not modify or resolve them.</p>
          <ul>
            <li>Total: {counts.total}</li>
            {MERGE_CONFLICT_KIND_LABELS.filter(([key]) => counts[key] > 0).map(([key, label]) => <li key={key}>{label}: {counts[key]}</li>)}
          </ul>
        </div>
        {notice}
        {abortAction}
      </div>;
      }
    case "resolvedPendingConfirmation":
      return <div className="merge-conflict-panel">
        <p className="muted">Git no longer reports unmerged entries, but ChatOMS has not confirmed or completed the merge.</p>
        {notice}
        <button className="button" onClick={onConfirm}>Confirm the staged merge resolution</button>
        {abortAction}
      </div>;
    case "restoredPendingAbortConfirmation":
      return <div className="merge-conflict-panel">
        <p className="muted">Git reports no merge in progress, and the original checkout already matches the base state it had before the merge started. This task has not yet been confirmed cancelled.</p>
        {notice}
        {abortAction}
      </div>;
    case "inconsistent":
      return <p className="inline-notice">The saved task and current Git merge state do not match. No merge action was attempted.</p>;
    case "unavailable":
      return <p className="inline-notice">The merge conflict state could not be verified safely. No merge action was attempted.</p>;
    default:
      return assertNever(state.result.outcome);
  }
}

/**
 * The single gate every merge-conflict action goes through, on the panel and
 * inside the confirmation dialogs alike. Actions are permitted only when the
 * runtime's shared lock has been read successfully, reports itself free, and
 * this page has not just started a write of its own. `undefined`, `loading`
 * and `error` all withhold the actions.
 */
function mergeConflictActionsAllowed(
  status: MergeConflictWriteStatusState | undefined,
  startedLocally: boolean,
): boolean {
  return status !== undefined && status.kind === "ready" && !status.running && !startedLocally;
}

/** Fixed, content-free copy: never a raw backend error string. */
const MERGE_CONFLICT_WRITE_BUSY_NOTICE =
  "A merge action is already processing for this task, or its status needs to be refreshed. This status updates automatically.";

function assertNever(_value: never): never {
  throw new Error("Unexpected merge conflict inspection outcome.");
}

export function formatTimestamp(value: number): string {
  if (!Number.isFinite(value)) return "Unknown";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
}
