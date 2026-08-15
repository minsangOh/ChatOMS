import { describe, expect, it } from "vitest";
import { isDiffContentHash, isRawUserDiffForReviewDto, isUserDiffApprovalDto } from "./user_diff_review";

const VALID_HASH = "a".repeat(64);

describe("diff content hash guard", () => {
  it("accepts a well-formed lowercase 64-character hex digest", () => {
    expect(isDiffContentHash(VALID_HASH)).toBe(true);
    expect(isDiffContentHash("0123456789abcdef".repeat(4))).toBe(true);
  });

  it("fail-closed rejects malformed digests", () => {
    for (const malformed of [
      "",
      "a".repeat(63),
      "a".repeat(65),
      "A".repeat(64),
      "g".repeat(64),
      null,
      undefined,
      1,
      {},
    ]) {
      expect(isDiffContentHash(malformed)).toBe(false);
    }
  });
});

describe("raw user diff for review DTO guard", () => {
  it("accepts a well-formed raw diff response", () => {
    expect(
      isRawUserDiffForReviewDto({
        diffText: "diff --git a/x b/x\n+line\n",
        diffContentHash: VALID_HASH,
      }),
    ).toBe(true);
  });

  it("accepts an empty diff text string (still a valid string)", () => {
    expect(
      isRawUserDiffForReviewDto({ diffText: "", diffContentHash: VALID_HASH }),
    ).toBe(true);
  });

  it("fail-closed rejects a non-string diffText", () => {
    for (const malformed of [
      { diffText: 1, diffContentHash: VALID_HASH },
      { diffText: null, diffContentHash: VALID_HASH },
      { diffContentHash: VALID_HASH },
    ]) {
      expect(isRawUserDiffForReviewDto(malformed)).toBe(false);
    }
  });

  it("fail-closed rejects a malformed diffContentHash", () => {
    for (const malformed of [
      { diffText: "x", diffContentHash: "not-hex" },
      { diffText: "x", diffContentHash: "A".repeat(64) },
      { diffText: "x", diffContentHash: 100 },
      { diffText: "x" },
    ]) {
      expect(isRawUserDiffForReviewDto(malformed)).toBe(false);
    }
  });

  it("fail-closed rejects an unexpected extra field or a malformed shape", () => {
    for (const malformed of [
      { diffText: "x", diffContentHash: VALID_HASH, path: "C:\\leaked" },
      {},
      null,
      "x",
      [],
    ]) {
      expect(isRawUserDiffForReviewDto(malformed)).toBe(false);
    }
  });
});

describe("user diff approval DTO guard", () => {
  it("accepts a well-formed approval response", () => {
    expect(isUserDiffApprovalDto({ approvedAtMs: 100 })).toBe(true);
  });

  it("fail-closed rejects a non-numeric approvedAtMs", () => {
    for (const malformed of [
      { approvedAtMs: "100" },
      { approvedAtMs: null },
      {},
    ]) {
      expect(isUserDiffApprovalDto(malformed)).toBe(false);
    }
  });

  it("fail-closed rejects a response that carries a raw diff field", () => {
    for (const malformed of [
      { approvedAtMs: 100, diffText: "diff --git a/x b/x" },
      { approvedAtMs: 100, diffContentHash: "a".repeat(64) },
    ]) {
      expect(isUserDiffApprovalDto(malformed)).toBe(false);
    }
  });
});
