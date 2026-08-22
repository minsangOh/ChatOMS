import { describe, expect, it } from "vitest";
import {
  isOperationRiskAssessmentStatusDto,
} from "./operation_risk_assessment";

const readiness = [
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
].map((riskCategory) => ({ riskCategory, approved: riskCategory === "dataMigration" }));

describe("operation risk assessment guard", () => {
  it("accepts incomplete, explicit-empty, non-empty, and safe failure status", () => {
    expect(isOperationRiskAssessmentStatusDto({
      assessmentRequired: true,
      declarationExists: false,
      selectedCategories: [],
      approvalReadiness: readiness,
      failureCategory: null,
    })).toBe(true);
    expect(isOperationRiskAssessmentStatusDto({
      assessmentRequired: false,
      declarationExists: true,
      selectedCategories: [],
      approvalReadiness: readiness,
      failureCategory: null,
    })).toBe(true);
    expect(isOperationRiskAssessmentStatusDto({
      assessmentRequired: false,
      declarationExists: true,
      selectedCategories: ["dataMigration"],
      approvalReadiness: readiness,
      failureCategory: null,
    })).toBe(true);
    expect(isOperationRiskAssessmentStatusDto({
      assessmentRequired: null,
      declarationExists: null,
      selectedCategories: [],
      approvalReadiness: [],
      failureCategory: "identityMismatch",
    })).toBe(true);
  });

  it.each([
    null,
    {},
    { assessmentRequired: "yes", declarationExists: false, selectedCategories: [], approvalReadiness: readiness, failureCategory: null },
    { assessmentRequired: true, declarationExists: true, selectedCategories: [], approvalReadiness: readiness, failureCategory: null },
    { assessmentRequired: true, declarationExists: false, selectedCategories: ["dataMigration"], approvalReadiness: readiness, failureCategory: null },
    { assessmentRequired: false, declarationExists: true, selectedCategories: ["unknown"], approvalReadiness: readiness, failureCategory: null },
    { assessmentRequired: false, declarationExists: true, selectedCategories: ["dataMigration", "dataMigration"], approvalReadiness: readiness, failureCategory: null },
    { assessmentRequired: false, declarationExists: true, selectedCategories: ["architectureChange"], approvalReadiness: readiness, failureCategory: null },
    { assessmentRequired: true, declarationExists: false, selectedCategories: [], approvalReadiness: readiness.slice(1), failureCategory: null },
    { assessmentRequired: true, declarationExists: false, selectedCategories: [], approvalReadiness: [...readiness, readiness[0]], failureCategory: null },
    { assessmentRequired: null, declarationExists: null, selectedCategories: [], approvalReadiness: [], failureCategory: "unknown" },
    { assessmentRequired: null, declarationExists: null, selectedCategories: ["dataMigration"], approvalReadiness: [], failureCategory: "invalidState" },
    { assessmentRequired: true, declarationExists: false, selectedCategories: [], approvalReadiness: readiness, failureCategory: null, path: "C:/secret" },
    { assessmentRequired: true, declarationExists: false, selectedCategories: [], approvalReadiness: readiness, failureCategory: null, digest: "secret" },
    { assessmentRequired: true, declarationExists: false, selectedCategories: [], approvalReadiness: readiness, failureCategory: null, stdout: "secret" },
    { assessmentRequired: true, declarationExists: false, selectedCategories: [], approvalReadiness: readiness, failureCategory: null, operation: "providerImplementation" },
  ])("rejects malformed or expanded response %#", (value) => {
    expect(isOperationRiskAssessmentStatusDto(value)).toBe(false);
  });
});
