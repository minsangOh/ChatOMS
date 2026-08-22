import { isRecord } from "./errors";
import type { HighRiskApprovalDto, HighRiskApprovalStatusDto, HighRiskCategory } from "./types";

export const HIGH_RISK_CATEGORIES: readonly HighRiskCategory[] = [
  "architectureChange",
  "databaseSchemaChange",
  "authenticationOrAuthorizationChange",
  "securityPolicyChange",
  "externalNetworkBehaviorAddition",
  "externalDataTransmissionAddition",
  "largeScaleFileMoveOrDeletion",
  "publicApiOrStorageFormatChange",
  "operatingSystemConfigurationChange",
  "administratorPrivilegesRequired",
  "breakingCompatibilityChange",
  "dataMigration",
  "difficultToRecoverChange",
];

const STATUS_KEYS = ["approved"] as const;
const APPROVAL_KEYS = ["riskCategory", "approvedAtMs"] as const;

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

export function isHighRiskCategory(value: unknown): value is HighRiskCategory {
  return typeof value === "string" && HIGH_RISK_CATEGORIES.includes(value as HighRiskCategory);
}

/// Fail-closed runtime guard: rejects anything other than an object with
/// exactly one boolean `approved` field, instead of coercing a truthy/falsy
/// value or defaulting a malformed response to `false`.
export function isHighRiskApprovalStatusDto(value: unknown): value is HighRiskApprovalStatusDto {
  return (
    isRecord(value) && hasExactKeys(value, STATUS_KEYS) && typeof value.approved === "boolean"
  );
}

/// Fail-closed runtime guard: rejects an unknown `riskCategory` literal, a
/// non-numeric `approvedAtMs`, or any extra/missing field.
export function isHighRiskApprovalDto(value: unknown): value is HighRiskApprovalDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, APPROVAL_KEYS) &&
    isHighRiskCategory(value.riskCategory) &&
    typeof value.approvedAtMs === "number"
  );
}
