import { isRecord } from "./errors";
import type {
  ApproveValidationCommandResultDto,
  CancelTestingDto,
  ValidationCommandApprovalStatusDto,
  ValidationCommandCandidateDto,
  ValidationCommandKind,
} from "./types";

const VALIDATION_COMMAND_KINDS: readonly ValidationCommandKind[] = [
  "format",
  "lint",
  "typecheck",
  "test",
  "build",
];
const CANDIDATE_KEYS = ["kind", "label"] as const;
const APPROVAL_STATUS_KEYS = ["approvedKinds"] as const;
const APPROVE_RESULT_KEYS = ["approvedKinds"] as const;
const CANCEL_TESTING_KEYS = ["requested"] as const;

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

function isValidationCommandKind(value: unknown): value is ValidationCommandKind {
  return typeof value === "string" && VALIDATION_COMMAND_KINDS.includes(value as ValidationCommandKind);
}

function isValidationCommandKindArray(value: unknown): value is ValidationCommandKind[] {
  return Array.isArray(value) && value.every(isValidationCommandKind);
}

function isValidationCommandCandidateDto(value: unknown): value is ValidationCommandCandidateDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, CANDIDATE_KEYS) &&
    isValidationCommandKind(value.kind) &&
    typeof value.label === "string"
  );
}

export function isValidationCommandCandidateDtoArray(
  value: unknown,
): value is ValidationCommandCandidateDto[] {
  return Array.isArray(value) && value.every(isValidationCommandCandidateDto);
}

export function isValidationCommandApprovalStatusDto(
  value: unknown,
): value is ValidationCommandApprovalStatusDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, APPROVAL_STATUS_KEYS) &&
    isValidationCommandKindArray(value.approvedKinds)
  );
}

export function isApproveValidationCommandResultDto(
  value: unknown,
): value is ApproveValidationCommandResultDto {
  return (
    isRecord(value) &&
    hasExactKeys(value, APPROVE_RESULT_KEYS) &&
    isValidationCommandKindArray(value.approvedKinds)
  );
}

export function isCancelTestingDto(value: unknown): value is CancelTestingDto {
  return (
    isRecord(value) && hasExactKeys(value, CANCEL_TESTING_KEYS) && typeof value.requested === "boolean"
  );
}
