import { isRecord } from "./errors";
import type { ProviderEligibilityDto } from "./types";

const WORK_KINDS = ["planning", "implementation", "review"] as const;
const PROVIDERS = ["claude", "codex"] as const;
const CAPABILITIES = ["supported", "unsupported", "unavailable"] as const;
const CONTRACTS = ["approved", "notApproved"] as const;
const BLOCKING_REASONS = [
  "capabilityUnavailable",
  "capabilityUnsupported",
  "contractNotApproved",
  "taskStateMismatch",
] as const;
const ENTRY_KEYS = [
  "workKind",
  "provider",
  "capability",
  "contract",
  "eligible",
  "stateAllowsWorkKind",
  "blockingReasons",
] as const;

export function isProviderEligibilityDtoArray(
  value: unknown,
): value is readonly ProviderEligibilityDto[] {
  if (
    !Array.isArray(value) ||
    value.length !== WORK_KINDS.length * PROVIDERS.length
  ) {
    return false;
  }
  const combinations = new Set<string>();
  for (const entry of value) {
    if (!isProviderEligibilityDto(entry)) {
      return false;
    }
    combinations.add(`${entry.workKind}:${entry.provider}`);
  }
  return combinations.size === WORK_KINDS.length * PROVIDERS.length;
}

function isProviderEligibilityDto(value: unknown): value is ProviderEligibilityDto {
  if (!isRecord(value)) {
    return false;
  }
  const keys = Object.keys(value);
  return (
    keys.length === ENTRY_KEYS.length &&
    keys.every((key) => ENTRY_KEYS.some((allowed) => allowed === key)) &&
    isAllowedString(value.workKind, WORK_KINDS) &&
    isAllowedString(value.provider, PROVIDERS) &&
    isAllowedString(value.capability, CAPABILITIES) &&
    isAllowedString(value.contract, CONTRACTS) &&
    typeof value.eligible === "boolean" &&
    typeof value.stateAllowsWorkKind === "boolean" &&
    Array.isArray(value.blockingReasons) &&
    value.blockingReasons.every((reason) => isAllowedString(reason, BLOCKING_REASONS))
  );
}

function isAllowedString(value: unknown, allowed: readonly string[]): boolean {
  return typeof value === "string" && allowed.includes(value);
}
