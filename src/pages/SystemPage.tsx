import { useCallback, useEffect, useState } from "react";
import { ErrorState } from "../components/ErrorState";
import { LoadingState } from "../components/LoadingState";
import { StatusBadge } from "../components/StatusBadge";
import { toFrontendError, type FrontendError } from "../ipc/errors";
import type { IpcClient } from "../ipc/client";
import type { BootstrapStatusDto, HealthState, SystemStatusDto } from "../ipc/types";

interface SystemPageProps {
  client: IpcClient;
}

type SystemPageState =
  | { kind: "loading" }
  | { kind: "error"; error: FrontendError }
  | {
      kind: "ready";
      system: SystemStatusDto;
      bootstrap: BootstrapStatusDto | null;
      version: string;
      health: HealthState;
      partialError: FrontendError | null;
    };

export function SystemPage({ client }: SystemPageProps) {
  const [requestId, setRequestId] = useState(0);
  const [state, setState] = useState<SystemPageState>({ kind: "loading" });

  useEffect(() => {
    let active = true;
    setState({ kind: "loading" });

    void Promise.allSettled([
      client.getVersion(),
      client.getHealth(),
      client.getSystemStatus(),
      client.getBootstrapStatus(),
    ]).then(([versionResult, healthResult, systemResult, bootstrapResult]) => {
      if (!active) {
        return;
      }
      if (systemResult.status === "rejected") {
        setState({ kind: "error", error: toFrontendError(systemResult.reason) });
        return;
      }

      const auxiliaryFailure = [versionResult, healthResult, bootstrapResult].find(
        (result) => result.status === "rejected",
      );
      setState({
        kind: "ready",
        system: systemResult.value,
        bootstrap: bootstrapResult.status === "fulfilled" ? bootstrapResult.value : null,
        version:
          versionResult.status === "fulfilled"
            ? versionResult.value.version
            : systemResult.value.applicationVersion,
        health:
          healthResult.status === "fulfilled"
            ? healthResult.value.status
            : systemResult.value.health,
        partialError:
          auxiliaryFailure?.status === "rejected"
            ? toFrontendError(auxiliaryFailure.reason)
            : null,
      });
    });

    return () => {
      active = false;
    };
  }, [client, requestId]);

  const retry = useCallback(() => {
    setRequestId((value) => value + 1);
  }, []);

  if (state.kind === "loading") {
    return <LoadingState message="Loading system status…" />;
  }
  if (state.kind === "error") {
    return <ErrorState error={state.error} onRetry={retry} />;
  }

  const { system, bootstrap } = state;
  return (
    <div className="page-stack">
      <header className="page-header">
        <div>
          <p className="eyebrow">Foundation status</p>
          <h1>System</h1>
          <p>Local readiness and Phase 1 platform capabilities.</p>
        </div>
        <StatusBadge status={state.health} />
      </header>

      {state.partialError ? (
        <div className="inline-notice" role="status">
          Some supplementary status is unavailable. {state.partialError.code}
        </div>
      ) : null}

      <section className="summary-grid" aria-label="System summary">
        <StatusItem label="Application version" value={state.version} />
        <StatusItem label="Overall health" status={state.health} />
        <StatusItem label="Storage" status={system.storageStatus} />
        <StatusItem label="Database" status={system.databaseStatus} />
        <StatusItem label="Logging" status={system.loggingStatus} />
        <StatusItem
          label="Bootstrap"
          status={bootstrap === null ? "notChecked" : bootstrap.ready ? "ready" : "unavailable"}
        />
      </section>

      <section className="content-card" aria-labelledby="active-task-heading">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Application lease</p>
            <h2 id="active-task-heading">Active task</h2>
          </div>
          <StatusBadge status={system.activeTaskStatus.status} />
        </div>
        {system.activeTaskStatus.status === "active" ? (
          <dl className="detail-list">
            <div>
              <dt>Task ID</dt>
              <dd className="identifier">{system.activeTaskStatus.taskId ?? "Unknown"}</dd>
            </div>
            <div>
              <dt>Lease acquired</dt>
              <dd>{formatTimestamp(system.activeTaskStatus.acquiredAtMs)}</dd>
            </div>
          </dl>
        ) : (
          <p className="muted">No active task currently holds the application lease.</p>
        )}
      </section>

      <section className="content-card" aria-labelledby="capabilities-heading">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Local interfaces</p>
            <h2 id="capabilities-heading">Capabilities</h2>
          </div>
        </div>
        <dl className="capability-list">
          <Capability label="Secure storage" status={system.capabilities.secureStorage} />
          <Capability label="Native permissions" status={system.capabilities.nativePermissions} />
          <Capability label="Git execution" status={system.capabilities.gitExecution} />
          <Capability label="Claude execution" status={system.capabilities.claudeExecution} />
          <Capability label="Codex execution" status={system.capabilities.codexExecution} />
          <Capability label="Updater" status={system.capabilities.updater} />
          <Capability label="Installer management" status={system.capabilities.installerManagement} />
        </dl>
      </section>
    </div>
  );
}

interface StatusItemProps {
  label: string;
  status?: string;
  value?: string;
}

function StatusItem({ label, status, value }: StatusItemProps) {
  return (
    <div className="summary-card">
      <span>{label}</span>
      {status === undefined ? <strong>{value}</strong> : <StatusBadge status={status} />}
    </div>
  );
}

function Capability({ label, status }: { label: string; status: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>
        <StatusBadge status={status} />
      </dd>
    </div>
  );
}

function formatTimestamp(value: number | null): string {
  if (value === null || !Number.isFinite(value)) {
    return "Unknown";
  }
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : new Intl.DateTimeFormat().format(date);
}
