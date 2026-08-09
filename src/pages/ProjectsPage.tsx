import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { EmptyState } from "../components/EmptyState";
import { ErrorState } from "../components/ErrorState";
import { LoadingState } from "../components/LoadingState";
import type { IpcClient } from "../ipc/client";
import { toFrontendError, type FrontendError } from "../ipc/errors";
import type { ProjectCandidateDto, ProjectDto, ProjectStatusDto, TaskIsolationDto } from "../ipc/types";

interface ProjectsPageProps { client: IpcClient; }
type ProjectsPageState = { kind: "loading" } | { kind: "error"; error: FrontendError } | { kind: "ready"; projects: ProjectDto[] };

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
  const createIsolation = async (projectId: string) => run(async () => {
    const isolation = await client.createIsolationTask(projectId);
    setIsolations((current) => ({ ...current, [projectId]: isolation }));
    setActiveTaskId(isolation.taskId);
  });
  const approveInit = async (projectId: string, isolation: TaskIsolationDto) => run(async () => {
    const next = await client.approveGitInitialization(isolation.taskId, isolation.taskVersion);
    setIsolations((current) => ({ ...current, [projectId]: next }));
  });
  const createWorktree = async (projectId: string, isolation: TaskIsolationDto) => run(async () => {
    const next = await client.createTaskWorktree(isolation.taskId, isolation.taskVersion);
    setIsolations((current) => ({ ...current, [projectId]: next }));
  });

  if (state.kind === "loading") return <LoadingState message="Loading projects" />;
  if (state.kind === "error") return <ErrorState error={state.error} onRetry={retry} />;

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
          <div className="card-actions"><button className="button button--secondary" disabled={busy} onClick={() => void run(async () => { const next = await client.getProjectGitStatus(project.id); setStatuses((current) => ({ ...current, [project.id]: next })); })}>Check Git status</button>{!isolation && activeTaskId === null && <button className="button" disabled={busy} onClick={() => void createIsolation(project.id)}>Create isolated task</button>}</div>
          {status && <p className="muted">{status.repositoryKind === "nonGit" ? "Git is not initialized." : status.repositoryStatus?.clean ? "Repository is clean." : "Repository is dirty; task isolation is blocked."}</p>}
          {isolation && <section className="isolation-panel" aria-label={`Isolation for ${project.name}`}><strong>{isolation.taskState}</strong><span className="identifier">{isolation.branchIdentity}</span>{isolation.blocker && <p className="inline-notice">{blockerMessage(isolation.blocker)}</p>}
            {isolation.taskState === "awaitingGitInitApproval" && <><p>Approval runs <code>git init</code>, stages files allowed by the existing .gitignore, and creates an initial snapshot commit. ChatOMS will not create or edit .gitignore or Git author settings.</p><button className="button" disabled={busy} onClick={() => void approveInit(project.id, isolation)}>Approve Git initialization</button></>}
            {(isolation.taskState === "projectValidated" || isolation.taskState === "gitInitialized") && <button className="button" disabled={busy} onClick={() => void createWorktree(project.id, isolation)}>Create managed worktree</button>}
            <button className="button button--secondary" disabled={busy} onClick={() => void run(async () => { const next = await client.getTaskIsolation(isolation.taskId); setIsolations((current) => ({ ...current, [project.id]: next })); })}>Refresh isolation</button>
          </section>}
        </li>;
      })}</ul>}
  </div>;
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

export function formatTimestamp(value: number): string {
  if (!Number.isFinite(value)) return "Unknown";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
}
