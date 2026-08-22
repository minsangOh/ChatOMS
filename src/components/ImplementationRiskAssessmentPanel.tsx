import { useState } from "react";
import { HIGH_RISK_CATEGORIES } from "../ipc/high_risk_approval";
import type {
  HighRiskCategory,
  OperationRiskAssessmentStatusDto,
} from "../ipc/types";

export type OperationRiskAssessmentLoadState =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; status: OperationRiskAssessmentStatusDto };

interface ImplementationRiskAssessmentPanelProps {
  state: OperationRiskAssessmentLoadState;
  busy: boolean;
  onDeclare(
    categories: readonly HighRiskCategory[],
    explicitEmpty: boolean,
  ): Promise<OperationRiskAssessmentStatusDto>;
  onRecorded(status: OperationRiskAssessmentStatusDto): void;
}

type AssessmentMode = "categories" | "empty" | null;

export function ImplementationRiskAssessmentPanel({
  state,
  busy,
  onDeclare,
  onRecorded,
}: ImplementationRiskAssessmentPanelProps) {
  const [mode, setMode] = useState<AssessmentMode>(null);
  const [selected, setSelected] = useState<readonly HighRiskCategory[]>([]);
  const [confirming, setConfirming] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [submissionError, setSubmissionError] = useState(false);

  if (state.kind === "loading") {
    return <AssessmentShell><p className="muted">Loading risk assessment status…</p></AssessmentShell>;
  }
  if (submissionError) {
    return <AssessmentShell><p className="inline-notice">Risk assessment could not be recorded safely. Refresh before continuing.</p></AssessmentShell>;
  }
  if (state.kind === "error" || state.status.failureCategory !== null) {
    return <AssessmentShell><p className="inline-notice">Risk assessment status could not be loaded safely. Refresh before continuing.</p></AssessmentShell>;
  }
  const status = state.status;
  if (status.declarationExists === true) {
    return <AssessmentShell>
      <p className="muted">Assessment recorded for the current task version. This immutable declaration cannot be changed.</p>
      {status.selectedCategories.length === 0
        ? <p><strong>No high-risk categories recorded</strong></p>
        : <ul>{status.selectedCategories.map((category) => <li key={category}>{categoryLabel(category)}</li>)}</ul>}
    </AssessmentShell>;
  }

  const selectedHasUnapproved = selected.some((category) =>
    status.approvalReadiness.find((entry) => entry.riskCategory === category)?.approved !== true,
  );
  const canReview = mode === "empty" || (mode === "categories" && selected.length > 0 && !selectedHasUnapproved);
  const controlsDisabled = busy || submitting;

  const chooseMode = (nextMode: Exclude<AssessmentMode, null>) => {
    setMode(nextMode);
    setSelected([]);
    setConfirming(false);
    setConfirmed(false);
  };
  const toggleCategory = (category: HighRiskCategory) => {
    setSelected((current) => current.includes(category)
      ? current.filter((candidate) => candidate !== category)
      : HIGH_RISK_CATEGORIES.filter((candidate) => candidate === category || current.includes(candidate)));
    setConfirming(false);
    setConfirmed(false);
  };
  const record = async () => {
    if (!confirmed || !canReview || mode === null || controlsDisabled) return;
    setSubmitting(true);
    try {
      const result = await onDeclare(selected, mode === "empty");
      if (result.failureCategory !== null || result.declarationExists !== true) {
        setSubmissionError(true);
        return;
      }
      onRecorded(result);
    } catch {
      setSubmissionError(true);
    } finally {
      setSubmitting(false);
    }
  };

  return <AssessmentShell>
    <p className="muted">Evaluate Provider Implementation using the fixed high-risk category vocabulary. No selection is made automatically.</p>
    {!confirming ? <>
      <fieldset className="risk-assessment-options">
        <legend>Assessment result</legend>
        <label className="checkbox-row">
          <input
            type="radio"
            name="implementation-risk-mode"
            checked={mode === "categories"}
            onChange={() => chooseMode("categories")}
            disabled={controlsDisabled}
          />
          Assess selected high-risk categories
        </label>
        <label className="checkbox-row">
          <input
            type="radio"
            name="implementation-risk-mode"
            checked={mode === "empty"}
            onChange={() => chooseMode("empty")}
            disabled={controlsDisabled}
          />
          No high-risk categories apply to this implementation
        </label>
      </fieldset>
      {mode === "categories" && <fieldset className="risk-assessment-options">
        <legend>High-risk categories</legend>
        <ul className="risk-assessment-category-list">{status.approvalReadiness.map((entry) => {
          const readinessId = `implementation-risk-${entry.riskCategory}-readiness`;
          return <li className="high-risk-approval-row" key={entry.riskCategory}>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={selected.includes(entry.riskCategory)}
              onChange={() => toggleCategory(entry.riskCategory)}
              disabled={controlsDisabled}
              aria-describedby={readinessId}
            />
            {categoryLabel(entry.riskCategory)}
          </label>
          <span className="muted" id={readinessId}>{entry.approved ? "Approved" : "Not approved"}</span>
        </li>;
        })}</ul>
      </fieldset>}
      {selectedHasUnapproved && <p className="inline-notice" role="status" aria-live="polite">Approve every selected category before finalizing.</p>}
      <div className="form-actions">
        <button
          className="button"
          type="button"
          disabled={controlsDisabled || !canReview}
          onClick={() => setConfirming(true)}
        >Review and confirm assessment</button>
      </div>
    </> : <>
      <h4>Confirm immutable assessment</h4>
      <p className="muted">This declaration is bound to the current task version. It does not replace any approval or start a provider.</p>
      <p>{mode === "empty" ? "No high-risk categories apply." : selected.map(categoryLabel).join(", ")}</p>
      <label className="checkbox-row">
        <input
          type="checkbox"
          checked={confirmed}
          onChange={(event) => setConfirmed(event.target.checked)}
          disabled={controlsDisabled}
        />
        I understand this assessment is immutable for the current task version and cannot be changed.
      </label>
      <div className="form-actions">
        <button className="button button--secondary" type="button" disabled={controlsDisabled} onClick={() => { setConfirming(false); setConfirmed(false); }}>Back</button>
        <button className="button" type="button" disabled={controlsDisabled || !confirmed} onClick={() => void record()}>Record immutable assessment</button>
      </div>
    </>}
  </AssessmentShell>;
}

function AssessmentShell({ children }: { children: React.ReactNode }) {
  return <section className="high-risk-approval-panel" aria-label="Provider Implementation risk assessment">
    <h3>Provider Implementation risk assessment</h3>
    {children}
  </section>;
}

function categoryLabel(category: HighRiskCategory): string {
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
  }
}
