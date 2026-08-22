import { isRecord } from "./errors";
import { HIGH_RISK_CATEGORIES, isHighRiskCategory } from "./high_risk_approval";
import type {
  HighRiskCategory,
  OperationRiskApprovalReadinessDto,
  OperationRiskAssessmentFailureCategory,
  OperationRiskAssessmentStatusDto,
} from "./types";

const STATUS_KEYS = [
  "assessmentRequired",
  "declarationExists",
  "selectedCategories",
  "approvalReadiness",
  "failureCategory",
] as const;
const READINESS_KEYS = ["riskCategory", "approved"] as const;
const FAILURE_CATEGORIES: readonly OperationRiskAssessmentFailureCategory[] = [
  "invalidInput",
  "notFound",
  "versionConflict",
  "invalidState",
  "activeLeaseConflict",
  "identityMismatch",
  "persistenceUnavailable",
  "internal",
];

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

function isFailureCategory(value: unknown): value is OperationRiskAssessmentFailureCategory {
  return typeof value === "string" && FAILURE_CATEGORIES.some((category) => category === value);
}

function isReadiness(value: unknown): value is OperationRiskApprovalReadinessDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, READINESS_KEYS) &&
    isHighRiskCategory(value.riskCategory) &&
    typeof value.approved === "boolean"
  );
}

function isUniqueCategoryList(value: unknown): value is readonly HighRiskCategory[] {
  return (
    Array.isArray(value) &&
    value.every(isHighRiskCategory) &&
    new Set(value).size === value.length
  );
}

function isCompleteReadiness(value: unknown): value is readonly OperationRiskApprovalReadinessDto[] {
  if (!Array.isArray(value) || value.length !== HIGH_RISK_CATEGORIES.length || !value.every(isReadiness)) {
    return false;
  }
  const categories = value.map((item) => item.riskCategory);
  return (
    new Set(categories).size === HIGH_RISK_CATEGORIES.length &&
    HIGH_RISK_CATEGORIES.every((category) => categories.includes(category))
  );
}

export function isOperationRiskAssessmentStatusDto(
  value: unknown,
): value is OperationRiskAssessmentStatusDto {
  if (!isRecord(value) || !hasExactKeys(value, STATUS_KEYS)) return false;
  if (value.failureCategory !== null) {
    return (
      isFailureCategory(value.failureCategory) &&
      value.assessmentRequired === null &&
      value.declarationExists === null &&
      Array.isArray(value.selectedCategories) &&
      value.selectedCategories.length === 0 &&
      Array.isArray(value.approvalReadiness) &&
      value.approvalReadiness.length === 0
    );
  }
  const selectedCategories = value.selectedCategories;
  const approvalReadiness = value.approvalReadiness;
  if (
    typeof value.assessmentRequired !== "boolean" ||
    typeof value.declarationExists !== "boolean" ||
    value.assessmentRequired === value.declarationExists ||
    !isUniqueCategoryList(selectedCategories) ||
    !isCompleteReadiness(approvalReadiness)
  ) {
    return false;
  }
  if (!value.declarationExists) return selectedCategories.length === 0;
  return selectedCategories.every((category) =>
    approvalReadiness.some((entry) =>
      entry.riskCategory === category && entry.approved,
    ),
  );
}
