import { isRecord } from "./errors";
import type { MergeConflictInspectionDto } from "./types";

const OUTCOMES = [
  "confirmedUnresolved",
  "resolvedPendingConfirmation",
  "restoredPendingAbortConfirmation",
  "inconsistent",
  "unavailable",
] as const;
const COUNT_KEYS = [
  "total",
  "bothModified",
  "bothAdded",
  "bothDeleted",
  "addedByUs",
  "addedByThem",
  "deletedByUs",
  "deletedByThem",
] as const;
const RESULT_KEYS = ["outcome", "counts"] as const;

function hasExactKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const keys = Object.keys(value);
  return keys.length === allowed.length && keys.every((key) => allowed.some((item) => item === key));
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isCounts(value: unknown): boolean {
  if (!isRecord(value) || !hasExactKeys(value, COUNT_KEYS)) return false;
  return COUNT_KEYS.every((key) => isNonNegativeInteger(value[key]));
}

export function isMergeConflictInspectionDto(value: unknown): value is MergeConflictInspectionDto {
  if (!isRecord(value) || !hasExactKeys(value, RESULT_KEYS) || !isCounts(value.counts)) return false;
  return typeof value.outcome === "string" && OUTCOMES.some((outcome) => outcome === value.outcome);
}

export function isNullableMergeConflictInspectionDto(
  value: unknown,
): value is MergeConflictInspectionDto | null {
  return value === null || isMergeConflictInspectionDto(value);
}
