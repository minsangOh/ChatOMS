import type { IpcClient } from "../ipc/client";
import { HIGH_RISK_CATEGORIES } from "../ipc/high_risk_approval";
import type {
  BootstrapStatusDto,
  HealthDto,
  ProjectDto,
  ProviderEligibilityDto,
  SystemStatusDto,
  VersionDto,
} from "../ipc/types";

export const version: VersionDto = { version: "0.1.0" };
export const health: HealthDto = { status: "healthy" };
export const bootstrapStatus: BootstrapStatusDto = {
  storageStatus: "ready",
  databaseStatus: "ready",
  loggingStatus: "ready",
  activeTaskStatus: { status: "none", taskId: null, acquiredAtMs: null },
  applicationVersion: "0.1.0",
  ready: true,
};
const failClosedEligibility: readonly ProviderEligibilityDto[] = [
  { workKind: "planning", provider: "claude", capability: "unavailable", contract: "notApproved", eligible: false, stateAllowsWorkKind: false, blockingReasons: ["contractNotApproved"] as const },
  { workKind: "planning", provider: "codex", capability: "unavailable", contract: "notApproved", eligible: false, stateAllowsWorkKind: false, blockingReasons: ["contractNotApproved"] as const },
  { workKind: "implementation", provider: "claude", capability: "unavailable", contract: "notApproved", eligible: false, stateAllowsWorkKind: false, blockingReasons: ["contractNotApproved"] as const },
  { workKind: "implementation", provider: "codex", capability: "unavailable", contract: "notApproved", eligible: false, stateAllowsWorkKind: false, blockingReasons: ["contractNotApproved"] as const },
  { workKind: "review", provider: "claude", capability: "unavailable", contract: "notApproved", eligible: false, stateAllowsWorkKind: false, blockingReasons: ["contractNotApproved"] as const },
  { workKind: "review", provider: "codex", capability: "unavailable", contract: "notApproved", eligible: false, stateAllowsWorkKind: false, blockingReasons: ["contractNotApproved"] as const },
];
export const systemStatus: SystemStatusDto = {
  applicationVersion: "0.1.0",
  health: "healthy",
  storageStatus: "ready",
  databaseStatus: "ready",
  loggingStatus: "ready",
  activeTaskStatus: { status: "none", taskId: null, acquiredAtMs: null },
  capabilities: {
    secureStorage: "supported",
    nativePermissions: "supported",
    gitExecution: "supported",
    claudeExecution: "unavailable",
    codexExecution: "unavailable",
    updater: "unavailable",
    installerManagement: "unavailable",
  },
};

export function createFakeClient(overrides: Partial<IpcClient> = {}): IpcClient {
  return {
    getVersion: async () => version,
    getHealth: async () => health,
    getSystemStatus: async () => systemStatus,
    getBootstrapStatus: async () => bootstrapStatus,
    getLegacyMigrationDiagnostic: async () => null,
    listProjects: async (): Promise<ProjectDto[]> => [],
    inspectProjectCandidate: async () => { throw new Error("not implemented in frontend tests"); },
    registerProject: async () => { throw new Error("not implemented in frontend tests"); },
    getProjectGitStatus: async () => { throw new Error("not implemented in frontend tests"); },
    createIsolationTask: async (_, brief) => { if (!brief) throw new Error("brief is required"); throw new Error("not implemented in frontend tests"); },
    getTaskIsolation: async () => { throw new Error("not implemented in frontend tests"); },
    approveGitInitialization: async () => { throw new Error("not implemented in frontend tests"); },
    createTaskWorktree: async () => { throw new Error("not implemented in frontend tests"); },
    getActiveTask: async () => null,
    getTask: async () => {
      throw new Error("not implemented in frontend tests");
    },
    listTaskHistory: async () => [],
    getProviderEligibility: async () => failClosedEligibility,
    setClaudeExecutablePath: async () => ({ displayPath: "%USERPROFILE%\\claude.exe", claudeExecution: "unavailable" as const }),
    refreshClaudeCapability: async () => ({ outcome: "completed" as const, claudeExecution: "unavailable" as const, codexExecution: "unsupported" as const }),
    startClaudePlanning: async () => { throw new Error("not implemented in frontend tests"); },
    cancelClaudePlanning: async () => { throw new Error("not implemented in frontend tests"); },
    getPlanningResult: async () => null,
    getPostMergeValidationResults: async () => [],
    getMergeConflictInspection: async () => null,
    getContextPackagePlanningReadiness: async () => ({ ready: false }),
    startClaudePlanningContextPackage: async () => { throw new Error("not implemented in frontend tests"); },
    startClaudeImplementation: async () => { throw new Error("not implemented in frontend tests"); },
    cancelClaudeImplementation: async () => { throw new Error("not implemented in frontend tests"); },
    getContextPackageImplementationReadiness: async () => ({ ready: false }),
    startClaudeImplementationContextPackage: async () => { throw new Error("not implemented in frontend tests"); },
    getValidationCommandCandidates: async () => [],
    getValidationCommandApprovalStatus: async () => ({ approvedKinds: [] }),
    approveValidationCommand: async () => { throw new Error("not implemented in frontend tests"); },
    getProjectRootValidationApprovalStatus: async () => ({ testApproved: false, buildApproved: false }),
    approveProjectRootValidation: async () => { throw new Error("not implemented in frontend tests"); },
    startValidationTesting: async () => { throw new Error("not implemented in frontend tests"); },
    cancelValidationTesting: async () => { throw new Error("not implemented in frontend tests"); },
    startClaudeReview: async () => { throw new Error("not implemented in frontend tests"); },
    cancelClaudeReview: async () => { throw new Error("not implemented in frontend tests"); },
    getReviewResult: async () => null,
    preparePlanningContextPackage: async () => { throw new Error("not implemented in frontend tests"); },
    prepareImplementationContextPackage: async () => { throw new Error("not implemented in frontend tests"); },
    prepareReviewContextPackage: async () => { throw new Error("not implemented in frontend tests"); },
    getContextPackageReviewReadiness: async () => ({ ready: false }),
    startClaudeReviewContextPackage: async () => { throw new Error("not implemented in frontend tests"); },
    getHighRiskApprovalStatus: async () => ({ approved: false }),
    approveHighRiskOperation: async () => { throw new Error("not implemented in frontend tests"); },
    getProviderImplementationRiskAssessmentStatus: async () => ({
      assessmentRequired: true,
      declarationExists: false,
      selectedCategories: [],
      approvalReadiness: HIGH_RISK_CATEGORIES.map((riskCategory) => ({
        riskCategory,
        approved: false,
      })),
      failureCategory: null,
    }),
    declareProviderImplementationRisk: async () => { throw new Error("not implemented in frontend tests"); },
    getUserDiffForReview: async () => { throw new Error("not implemented in frontend tests"); },
    approveUserDiff: async () => { throw new Error("not implemented in frontend tests"); },
    approveUserDiffAndStartMerge: async () => { throw new Error("not implemented in frontend tests"); },
    confirmManualResolutionAndStartMergeContinue: async () => { throw new Error("not implemented in frontend tests"); },
    confirmMergeAbortAndStart: async () => { throw new Error("not implemented in frontend tests"); },
    // Default: no merge-conflict write in flight. Tests that care about the
    // gate override this explicitly.
    getMergeConflictWriteStatus: async () => ({ running: false }),
    ...overrides,
  };
}
