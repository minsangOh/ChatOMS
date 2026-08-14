import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { EmptyState } from "../components/EmptyState";
import { ErrorState } from "../components/ErrorState";
import { LoadingState } from "../components/LoadingState";
import type { IpcClient } from "../ipc/client";
import { FrontendError, toFrontendError } from "../ipc/errors";
import type { ApproveValidationCommandInput, EligibilityBlockingReason, PlanningResultDto, ProjectCandidateDto, ProjectDto, ProjectStatusDto, ProviderEligibilityDto, ReviewResultDto, TaskBriefInput, TaskIsolationDto, ValidationCommandApprovalStatusDto, ValidationCommandCandidateDto, ValidationCommandKind } from "../ipc/types";

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
  const [planningResults, setPlanningResults] = useState<Record<string, PlanningResultLoadState>>({});
  const [reviewResults, setReviewResults] = useState<Record<string, ReviewResultLoadState>>({});
  const [validationCandidates, setValidationCandidates] = useState<Record<string, ValidationCandidatesLoadState>>({});
  const [validationApprovals, setValidationApprovals] = useState<Record<string, ValidationApprovalLoadState>>({});
  const [validationForm, setValidationForm] = useState<ValidationCommandForm>(emptyValidationCommandForm);
  const [testingRuns, setTestingRuns] = useState<Record<string, boolean>>({});
  const [reviewRuns, setReviewRuns] = useState<Record<string, boolean>>({});

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
    setValidationForm(emptyValidationCommandForm);
  }, [activeTaskId]);

  useEffect(() => {
    const activeExecutionEntries = Object.entries(isolations).filter(
      ([, isolation]) => isolation.taskState === "planning" || isolation.taskState === "implementing" || isolation.taskState === "testing" || isolation.taskState === "reviewing",
    );
    if (activeExecutionEntries.length === 0) return;
    const interval = setInterval(() => {
      void Promise.all(
        activeExecutionEntries.map(async ([projectId, isolation]) => {
          const next = await client.getTaskIsolation(isolation.taskId);
          setIsolations((current) => ({ ...current, [projectId]: next }));
        }),
      ).catch(() => {});
    }, 2000);
    return () => clearInterval(interval);
  }, [client, isolations]);

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
              return <div className="planning-panel">
                {entry && !eligible && <p className="inline-notice">{entry.blockingReasons.map(eligibilityBlockerMessage).join(" ")}</p>}
                <button className="button" disabled={busy || !eligible} onClick={() => setConsentDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "planning" })}>Start Claude Planning</button>
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
                return <div className="planning-panel">
                  {entry && !eligible && <p className="inline-notice">{entry.blockingReasons.map(eligibilityBlockerMessage).join(" ")}</p>}
                  <button className="button" disabled={busy || !eligible} onClick={() => setConsentDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "implementation" })}>Start Claude Implementation</button>
                </div>;
              })()}
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
              return <div className="review-panel" aria-label="Claude Review">
                {running ? <>
                  <p className="muted">Claude Review is analyzing the changes in this task's isolated worktree. This may take a few minutes.</p>
                  <button className="button button--secondary" disabled={busy} onClick={() => void cancelReview(isolation.taskId)}>Cancel review</button>
                </> : <>
                  {entry && !eligible && <p className="inline-notice">{entry.blockingReasons.map(eligibilityBlockerMessage).join(" ")}</p>}
                  <button className="button" disabled={busy || !eligible} onClick={() => setConsentDialog({ projectId: project.id, taskId: isolation.taskId, taskVersion: isolation.taskVersion, workKind: "review" })}>Start Claude Review</button>
                </>}
              </div>;
            })()}
            {isolation.taskState === "awaitingUserDiffApproval" && <div className="review-panel" aria-label="Claude Review result">
              <p className="muted">Claude Review finished. The review is awaiting your decision on the diff.</p>
              {renderReviewResult(reviewResults[isolation.taskId])}
            </div>}
            {isolation.taskState === "completed" && <p className="muted">This task is completed. Its active task lease has been released.</p>}
            {isolation.taskState === "recoveryRequired" && <p className="muted">This task requires manual recovery before continuing. Review the task before proceeding.</p>}
            {isolation.taskState === "failed" && <p className="inline-notice">Claude Planning failed. Review the task before retrying.</p>}
            {isolation.taskState === "cancelled" && <p className="muted">Claude Planning was cancelled.</p>}
            <button className="button button--secondary" disabled={busy} onClick={() => void run(async () => { const next = await client.getTaskIsolation(isolation.taskId); setIsolations((current) => ({ ...current, [project.id]: next })); })}>Refresh isolation</button>
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

export function formatTimestamp(value: number): string {
  if (!Number.isFinite(value)) return "Unknown";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
}
