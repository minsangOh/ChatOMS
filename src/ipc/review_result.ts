import { isRecord } from "./errors";
import type { CancelReviewDto, ReviewResultDto } from "./types";

const OUTCOMES = ["completed", "failed", "cancelled", "recoveryRequired"] as const;
const ENTRY_KEYS = [
  "outcome",
  "exitCode",
  "turnCount",
  "startedAtMs",
  "completedAtMs",
  "reviewText",
] as const;
const CANCEL_REVIEW_KEYS = ["requested"] as const;

export function isNullableReviewResultDto(
  value: unknown,
): value is ReviewResultDto | null {
  return value === null || isReviewResultDto(value);
}

function isReviewResultDto(value: unknown): value is ReviewResultDto {
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
    isNullableString(value.reviewText)
  );
}

export function isCancelReviewDto(value: unknown): value is CancelReviewDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === CANCEL_REVIEW_KEYS.length &&
    keys.every((key) => CANCEL_REVIEW_KEYS.some((allowed) => allowed === key)) &&
    typeof value.requested === "boolean"
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
