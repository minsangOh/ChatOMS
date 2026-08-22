import { isRecord } from "./errors";
import type { MergeConflictWriteStatusDto } from "./types";

const MERGE_CONFLICT_WRITE_STATUS_KEYS = ["running"] as const;

function hasExactKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const keys = Object.keys(value);
  return keys.length === allowed.length && keys.every((key) => allowed.some((item) => item === key));
}

/**
 * Fail-closed guard for the content-free merge-conflict write status.
 *
 * The exact-key check is the point, not a formality: this response gates
 * whether the merge-continue and merge-abort actions are shown, so a payload
 * that arrived carrying a `path`, `digest`, `stdout`, or `operation` field is
 * not something to read `running` out of and move on from — it is a
 * contract violation, and the caller must treat it exactly like a failed
 * request (no actions shown).
 */
export function isMergeConflictWriteStatusDto(
  value: unknown,
): value is MergeConflictWriteStatusDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, MERGE_CONFLICT_WRITE_STATUS_KEYS) &&
    typeof value.running === "boolean"
  );
}
