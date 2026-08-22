import { isRecord } from "./errors";
import type { MergeAbortStartDto } from "./types";

const MERGE_ABORT_START_KEYS = ["started"] as const;

function hasExactKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const keys = Object.keys(value);
  return keys.length === allowed.length && keys.every((key) => allowed.some((item) => item === key));
}

export function isMergeAbortStartDto(value: unknown): value is MergeAbortStartDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, MERGE_ABORT_START_KEYS) &&
    typeof value.started === "boolean"
  );
}
