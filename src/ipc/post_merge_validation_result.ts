import { isRecord } from "./errors";
import type { PostMergeValidationResultDto } from "./types";

const COMMAND_KINDS = ["test", "build"] as const;
const OUTCOMES = [
  "success",
  "exitFailure",
  "timedOut",
  "stdoutBoundExceeded",
  "bindingRejected",
  "cancelled",
  "uncertain",
] as const;
const RESULT_KEYS = [
  "commandKind",
  "attemptSequence",
  "outcome",
  "exitCode",
  "safeSummary",
  "startedAtMs",
  "completedAtMs",
] as const;

function hasExactKeys(value: Record<string, unknown>): boolean {
  const keys = Object.keys(value);
  return keys.length === RESULT_KEYS.length && keys.every((key) => RESULT_KEYS.some((allowed) => allowed === key));
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isNullableInteger(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isInteger(value));
}

function isPostMergeValidationResultDto(value: unknown): value is PostMergeValidationResultDto {
  if (!isRecord(value) || !hasExactKeys(value)) return false;
  return (
    typeof value.commandKind === "string" && COMMAND_KINDS.some((kind) => kind === value.commandKind) &&
    isNonNegativeInteger(value.attemptSequence) &&
    value.attemptSequence >= 1 &&
    typeof value.outcome === "string" && OUTCOMES.some((outcome) => outcome === value.outcome) &&
    isNullableInteger(value.exitCode) &&
    typeof value.safeSummary === "string" && value.safeSummary.length > 0 && value.safeSummary.length <= 2000 &&
    isNonNegativeInteger(value.startedAtMs) &&
    isNonNegativeInteger(value.completedAtMs) &&
    value.completedAtMs >= value.startedAtMs
  );
}

export function isPostMergeValidationResultDtoArray(
  value: unknown,
): value is PostMergeValidationResultDto[] {
  return Array.isArray(value) && value.every(isPostMergeValidationResultDto);
}
