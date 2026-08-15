import { isRecord } from "./errors";
import type { RawUserDiffForReviewDto, UserDiffApprovalDto } from "./types";

const RAW_DIFF_KEYS = ["diffText", "diffContentHash"] as const;
const APPROVAL_KEYS = ["approvedAtMs"] as const;
const HEX_64_PATTERN = /^[0-9a-f]{64}$/;

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

export function isDiffContentHash(value: unknown): value is string {
  return typeof value === "string" && HEX_64_PATTERN.test(value);
}

/// Fail-closed runtime guard for the ONLY response shape in this codebase
/// that carries raw repository diff content. Rejects a non-string
/// `diffText`, a malformed (wrong length/casing/non-hex) `diffContentHash`,
/// or any extra/missing field -- this is the dedicated guard for the
/// dedicated `getUserDiffForReview` method only, never reused for a
/// generic response cache.
export function isRawUserDiffForReviewDto(value: unknown): value is RawUserDiffForReviewDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, RAW_DIFF_KEYS) &&
    typeof value.diffText === "string" &&
    isDiffContentHash(value.diffContentHash)
  );
}

/// Fail-closed runtime guard: rejects a non-numeric `approvedAtMs` or any
/// extra field -- in particular, a response carrying a raw diff field is
/// rejected here since it would fail the exact-key-set check.
export function isUserDiffApprovalDto(value: unknown): value is UserDiffApprovalDto {
  return (
    isRecord(value) && hasExactKeys(value, APPROVAL_KEYS) && typeof value.approvedAtMs === "number"
  );
}
