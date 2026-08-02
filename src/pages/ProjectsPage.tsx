import { useCallback, useEffect, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { ErrorState } from "../components/ErrorState";
import { LoadingState } from "../components/LoadingState";
import type { IpcClient } from "../ipc/client";
import { toFrontendError, type FrontendError } from "../ipc/errors";
import type { ProjectDto } from "../ipc/types";

interface ProjectsPageProps {
  client: IpcClient;
}

type ProjectsPageState =
  | { kind: "loading" }
  | { kind: "error"; error: FrontendError }
  | { kind: "ready"; projects: ProjectDto[] };

export function ProjectsPage({ client }: ProjectsPageProps) {
  const [requestId, setRequestId] = useState(0);
  const [state, setState] = useState<ProjectsPageState>({ kind: "loading" });

  useEffect(() => {
    let active = true;
    setState({ kind: "loading" });
    void client.listProjects().then(
      (projects) => {
        if (active) {
          setState({ kind: "ready", projects });
        }
      },
      (error: unknown) => {
        if (active) {
          setState({ kind: "error", error: toFrontendError(error) });
        }
      },
    );
    return () => {
      active = false;
    };
  }, [client, requestId]);

  const retry = useCallback(() => {
    setRequestId((value) => value + 1);
  }, []);

  if (state.kind === "loading") {
    return <LoadingState message="Loading projects…" />;
  }
  if (state.kind === "error") {
    return <ErrorState error={state.error} onRetry={retry} />;
  }

  return (
    <div className="page-stack">
      <header className="page-header">
        <div>
          <p className="eyebrow">Registered workspaces</p>
          <h1>Projects</h1>
          <p>Read-only project metadata available to the Phase 1 foundation.</p>
        </div>
        <span className="count-label">{state.projects.length} total</span>
      </header>

      {state.projects.length === 0 ? (
        <EmptyState
          title="No projects"
          description="No projects have been registered yet."
        />
      ) : (
        <ul className="project-list" aria-label="Projects">
          {state.projects.map((project) => (
            <li className="project-card" key={project.id}>
              <h2>{project.name}</h2>
              <p className="identifier">{project.id}</p>
              <dl className="detail-list detail-list--compact">
                <div>
                  <dt>Created</dt>
                  <dd>{formatTimestamp(project.createdAtMs)}</dd>
                </div>
                <div>
                  <dt>Updated</dt>
                  <dd>{formatTimestamp(project.updatedAtMs)}</dd>
                </div>
              </dl>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function formatTimestamp(value: number): string {
  if (!Number.isFinite(value)) {
    return "Unknown";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "Unknown";
  }
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}
