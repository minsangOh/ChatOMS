import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { ErrorState } from "../components/ErrorState";
import { LoadingState } from "../components/LoadingState";
import { StatusBadge } from "../components/StatusBadge";
import { toFrontendError, type FrontendError } from "../ipc/errors";
import type { IpcClient } from "../ipc/client";
import type {
  BootstrapStatusDto,
  CapabilityStatus,
  HealthState,
  LegacyMigrationDiagnosticDto,
  RefreshOutcome,
  SystemStatusDto,
} from "../ipc/types";

interface SystemPageProps {
  client: IpcClient;
}

type SystemPageState =
  | { kind: "loading" }
  | { kind: "error"; error: FrontendError }
  | { kind: "legacy"; diagnostic: LegacyMigrationDiagnosticDto }
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
      client.getLegacyMigrationDiagnostic(),
    ]).then(([versionResult, healthResult, systemResult, bootstrapResult, diagnosticResult]) => {
      if (!active) {
        return;
      }
      if (systemResult.status === "rejected") {
        if (
          diagnosticResult.status === "fulfilled" &&
          diagnosticResult.value !== null
        ) {
          setState({ kind: "legacy", diagnostic: diagnosticResult.value });
          return;
        }
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
  if (state.kind === "legacy") {
    return (
      <div className="page-stack">
        <header className="page-header">
          <div>
            <p className="eyebrow">Migration review required</p>
            <h1>Legacy project verification stopped</h1>
            <p>ChatOMS did not merge, remove, or guess this project identity.</p>
          </div>
          <StatusBadge status="unavailable" />
        </header>
        <section className="content-card" aria-labelledby="legacy-project-heading">
          <h2 id="legacy-project-heading">Project requiring review</h2>
          <dl className="detail-list">
            <div>
              <dt>Project ID</dt>
              <dd className="identifier">{state.diagnostic.projectId}</dd>
            </div>
            <div>
              <dt>Display path</dt>
              <dd>{state.diagnostic.displayPath}</dd>
            </div>
            <div>
              <dt>Reason</dt>
              <dd>{state.diagnostic.reasonCode}</dd>
            </div>
          </dl>
          <p className="muted">Resolve the path or duplicate identity outside ChatOMS, then restart the app.</p>
        </section>
      </div>
    );
  }

  const { system, bootstrap } = state;
  return (
    <div className="page-stack">
      <header className="page-header">
        <div>
          <p className="eyebrow">Foundation status</p>
          <h1>System</h1>
          <p>Local readiness and Phase 2 platform capabilities.</p>
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

      <ProviderSection client={client} initialClaudeStatus={system.capabilities.claudeExecution} />

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

interface ProviderSectionProps {
  client: IpcClient;
  initialClaudeStatus: CapabilityStatus;
}

function ProviderSection({ client, initialClaudeStatus }: ProviderSectionProps) {
  const [displayPath, setDisplayPath] = useState<string | null>(null);
  const [claudeStatus, setClaudeStatus] = useState<CapabilityStatus>(initialClaudeStatus);
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [saveError, setSaveError] = useState<FrontendError | null>(null);
  const [refreshError, setRefreshError] = useState<FrontendError | null>(null);
  const [refreshOutcome, setRefreshOutcome] = useState<RefreshOutcome | null>(null);

  const handleChoose = useCallback(async () => {
    setSaveError(null);
    setRefreshOutcome(null);
    const selected = await open({
      multiple: false,
      directory: false,
      title: "Choose Claude executable",
    });
    if (selected === null) return;
    setSaving(true);
    try {
      const result = await client.setClaudeExecutablePath(selected);
      setDisplayPath(result.displayPath);
      setClaudeStatus(result.claudeExecution);
    } catch (error: unknown) {
      setSaveError(toFrontendError(error));
    } finally {
      setSaving(false);
    }
  }, [client]);

  const handleRefresh = useCallback(async () => {
    setRefreshError(null);
    setRefreshOutcome(null);
    setRefreshing(true);
    try {
      const result = await client.refreshClaudeCapability();
      setRefreshOutcome(result.outcome);
      setClaudeStatus(result.claudeExecution);
    } catch (error: unknown) {
      setRefreshError(toFrontendError(error));
    } finally {
      setRefreshing(false);
    }
  }, [client]);

  return (
    <section className="content-card" aria-labelledby="provider-heading">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Provider configuration</p>
          <h2 id="provider-heading">Claude executable</h2>
        </div>
        <StatusBadge status={claudeStatus} />
      </div>
      <dl className="detail-list">
        <div>
          <dt>Executable path</dt>
          <dd>{displayPath ?? "Not configured"}</dd>
        </div>
      </dl>
      <div className="action-row">
        <button type="button" onClick={handleChoose} disabled={saving || refreshing}>
          {saving ? "Saving…" : "Choose Claude executable"}
        </button>
        <button type="button" onClick={handleRefresh} disabled={saving || refreshing}>
          {refreshing ? "Refreshing…" : "Refresh"}
        </button>
      </div>
      {saveError ? (
        <p className="inline-error" role="alert">{saveError.message}</p>
      ) : null}
      {refreshError ? (
        <p className="inline-error" role="alert">{refreshError.message}</p>
      ) : null}
      {refreshOutcome === "conflict" ? (
        <p className="inline-notice" role="status">
          다른 새로고침이 진행 중입니다. 잠시 후 다시 시도하세요.
        </p>
      ) : null}
      {refreshOutcome === "superseded" ? (
        <p className="inline-notice" role="status">
          실행 파일 경로가 변경되어 결과를 적용하지 않았습니다. 새로고침을 다시 실행하세요.
        </p>
      ) : null}
    </section>
  );
}

function formatTimestamp(value: number | null): string {
  if (value === null || !Number.isFinite(value)) {
    return "Unknown";
  }
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : new Intl.DateTimeFormat().format(date);
}
