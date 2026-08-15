import { describe, expect, it } from "vitest";
import {
  HIGH_RISK_CATEGORIES,
  isHighRiskApprovalDto,
  isHighRiskApprovalStatusDto,
  isHighRiskCategory,
} from "./high_risk_approval";

describe("high-risk category guard", () => {
  it("has exactly the 13 fixed categories", () => {
    expect(HIGH_RISK_CATEGORIES).toHaveLength(13);
    expect(new Set(HIGH_RISK_CATEGORIES).size).toBe(13);
  });

  it("accepts every one of the 13 fixed categories", () => {
    for (const category of HIGH_RISK_CATEGORIES) {
      expect(isHighRiskCategory(category)).toBe(true);
    }
  });

  it("fail-closed rejects an unknown or malformed category value", () => {
    for (const malformed of [
      "notACategory",
      "ArchitectureChange",
      "",
      null,
      undefined,
      1,
      {},
      [],
    ]) {
      expect(isHighRiskCategory(malformed)).toBe(false);
    }
  });
});

describe("high-risk approval status DTO guard", () => {
  it("accepts a well-formed status", () => {
    expect(isHighRiskApprovalStatusDto({ approved: true })).toBe(true);
    expect(isHighRiskApprovalStatusDto({ approved: false })).toBe(true);
  });

  it("fail-closed rejects a non-boolean approved field", () => {
    for (const malformed of [{ approved: "true" }, { approved: 1 }, { approved: null }]) {
      expect(isHighRiskApprovalStatusDto(malformed)).toBe(false);
    }
  });

  it("fail-closed rejects an unexpected extra field or a missing field", () => {
    for (const malformed of [{ approved: true, riskCategory: "dataMigration" }, {}, null, "x"]) {
      expect(isHighRiskApprovalStatusDto(malformed)).toBe(false);
    }
  });
});

describe("high-risk approval DTO guard", () => {
  it("accepts a well-formed approval for every one of the 13 categories", () => {
    for (const category of HIGH_RISK_CATEGORIES) {
      expect(
        isHighRiskApprovalDto({ riskCategory: category, approvedAtMs: 100 }),
      ).toBe(true);
    }
  });

  it("fail-closed rejects an unknown risk category", () => {
    expect(
      isHighRiskApprovalDto({ riskCategory: "notACategory", approvedAtMs: 100 }),
    ).toBe(false);
  });

  it("fail-closed rejects a non-numeric approvedAtMs", () => {
    for (const malformed of [
      { riskCategory: "dataMigration", approvedAtMs: "100" },
      { riskCategory: "dataMigration", approvedAtMs: null },
      { riskCategory: "dataMigration" },
    ]) {
      expect(isHighRiskApprovalDto(malformed)).toBe(false);
    }
  });

  it("fail-closed rejects an unexpected extra field", () => {
    expect(
      isHighRiskApprovalDto({
        riskCategory: "dataMigration",
        approvedAtMs: 100,
        path: "C:\\leaked",
      }),
    ).toBe(false);
  });
});
