import { isRecord } from "./errors";
import type { PlanningResultDto } from "./types";

const OUTCOMES = ["completed", "failed", "cancelled", "recoveryRequired"] as const;
const ENTRY_KEYS = [
  "outcome",
  "exitCode",
  "turnCount",
  "startedAtMs",
  "completedAtMs",
  "planText",
] as const;

export function isNullablePlanningResultDto(
  value: unknown,
): value is PlanningResultDto | null {
  return value === null || isPlanningResultDto(value);
}

function isPlanningResultDto(value: unknown): value is PlanningResultDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === ENTRY_KEYS.length &&
    keys.every((key) => ENTRY_KEYS.some((allowed) => allowed === key)) &&
    isAllowedString(value.outcome, OUTCOMES) &&
    isNullableNumber(value.exitCode) &&
    isNullableNumber(value.turnCount) &&
    typeof value.startedAtMs === "number" &&
    typeof value.completedAtMs === "number" &&
    isNullableString(value.planText)
  );
}

function isAllowedString(value: unknown, allowed: readonly string[]): boolean {
  return typeof value === "string" && allowed.includes(value);
}

function isNullableNumber(value: unknown): value is number | null {
  return value === null || typeof value === "number";
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}
