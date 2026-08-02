type StatusTone = "positive" | "warning" | "negative" | "neutral";

const LABELS: Record<string, string> = {
  healthy: "Healthy",
  ready: "Ready",
  upgraded: "Upgraded",
  supported: "Supported",
  active: "Active",
  degraded: "Degraded",
  warning: "Warning",
  unavailable: "Unavailable",
  insecure: "Insecure",
  incompatible: "Incompatible",
  error: "Error",
  unsupported: "Unsupported",
  notChecked: "Not checked",
  migrationRequired: "Migration required",
  none: "None",
};

const POSITIVE = new Set(["healthy", "ready", "upgraded", "supported", "active"]);
const WARNING = new Set(["degraded", "warning", "migrationRequired"]);
const NEGATIVE = new Set(["unavailable", "insecure", "incompatible", "error"]);

interface StatusBadgeProps {
  status: string;
  label?: string;
}

export function StatusBadge({ status, label }: StatusBadgeProps) {
  return (
    <span className={`status-badge status-badge--${toneFor(status)}`}>
      <span className="status-badge__mark" aria-hidden="true" />
      {label ?? LABELS[status] ?? status}
    </span>
  );
}

function toneFor(status: string): StatusTone {
  if (POSITIVE.has(status)) {
    return "positive";
  }
  if (WARNING.has(status)) {
    return "warning";
  }
  if (NEGATIVE.has(status)) {
    return "negative";
  }
  return "neutral";
}
