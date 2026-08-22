import { invoke } from "@tauri-apps/api/core";
import { FrontendError, isRecord, toFrontendError } from "./errors";
import { isHighRiskApprovalDto, isHighRiskApprovalStatusDto } from "./high_risk_approval";
import { isMergeAbortStartDto } from "./merge_abort";
import { isNullablePlanningResultDto } from "./planning_result";
import { isPostMergeValidationResultDtoArray } from "./post_merge_validation_result";
import { isNullableMergeConflictInspectionDto } from "./merge_conflict_inspection";
import { isProviderEligibilityDtoArray } from "./provider_eligibility";
import { isCancelReviewDto, isNullableReviewResultDto } from "./review_result";
import { isRawUserDiffForReviewDto, isUserDiffApprovalDto } from "./user_diff_review";
import {
  isApproveValidationCommandResultDto,
  isCancelTestingDto,
  isProjectRootValidationApprovalStatusDto,
  isValidationCommandApprovalStatusDto,
  isValidationCommandCandidateDtoArray,
} from "./validation_command";
import {
  isContextPackageImplementationReadinessDto,
  isContextPackagePlanningReadinessDto,
  isContextPackagePreparationDto,
  isContextPackageReviewReadinessDto,
} from "./types";
import type {
  ActiveTaskDto,
  ActiveTaskStatusDto,
  ApproveValidationCommandInput,
  ApproveValidationCommandResultDto,
  ApproveProjectRootValidationInput,
  BootstrapStatusDto,
  CancelImplementationDto,
  CancelPlanningDto,
  CancelReviewDto,
  CancelTestingDto,
  CapabilityDto,
  ContextPackageImplementationReadinessDto,
  ContextPackagePlanningReadinessDto,
  ContextPackagePreparationDto,
  ContextPackageReviewReadinessDto,
  DatabaseStatus,
  HealthDto,
  HealthState,
  HighRiskApprovalDto,
  HighRiskApprovalStatusDto,
  HighRiskCategory,
  LoggingStatus,
  LegacyMigrationDiagnosticDto,
  MergeAbortStartDto,
  PlanningResultDto,
  PostMergeValidationResultDto,
  MergeConflictInspectionDto,
  ProjectDto,
  ProjectRootValidationApprovalStatusDto,
  ProviderEligibilityDto,
  ProjectCandidateDto,
  ProjectStatusDto,
  RawUserDiffForReviewDto,
  RefreshClaudeCapabilityDto,
  RefreshOutcome,
  ReviewResultDto,
  SetClaudeExecutablePathDto,
  TaskIsolationDto,
  StorageStatus,
  SystemStatusDto,
  TaskBriefDto,
  TaskBriefInput,
  TaskDto,
  TaskState,
  TaskTransitionDto,
  UserDiffApprovalDto,
  ValidationCommandApprovalStatusDto,
  ValidationCommandCandidateDto,
  VersionDto,
} from "./types";

export const IPC_COMMANDS = {
  getVersion: "get_version",
  getHealth: "get_health",
  getSystemStatus: "get_system_status",
  getBootstrapStatus: "get_bootstrap_status",
  getLegacyMigrationDiagnostic: "get_legacy_migration_diagnostic",
  listProjects: "list_projects",
  inspectProjectCandidate: "inspect_project_candidate",
  registerProject: "register_project",
  getProjectGitStatus: "get_project_git_status",
  createIsolationTask: "create_isolation_task",
  getTaskIsolation: "get_task_isolation",
  approveGitInitialization: "approve_git_initialization",
  createTaskWorktree: "create_task_worktree",
  getActiveTask: "get_active_task",
  getTask: "get_task",
  listTaskHistory: "list_task_history",
  getProviderEligibility: "get_provider_eligibility",
  setClaudeExecutablePath: "set_claude_executable_path",
  refreshClaudeCapability: "refresh_claude_capability",
  startClaudePlanning: "start_claude_planning",
  cancelClaudePlanning: "cancel_claude_planning",
  getPlanningResult: "get_planning_result",
  getPostMergeValidationResults: "get_post_merge_validation_results",
  getMergeConflictInspection: "get_merge_conflict_inspection",
  getContextPackagePlanningReadiness: "get_context_package_planning_readiness",
  startClaudePlanningContextPackage: "start_claude_planning_context_package",
  startClaudeImplementation: "start_claude_implementation",
  cancelClaudeImplementation: "cancel_claude_implementation",
  getContextPackageImplementationReadiness: "get_context_package_implementation_readiness",
  startClaudeImplementationContextPackage: "start_claude_implementation_context_package",
  startValidationTesting: "start_validation_testing",
  cancelValidationTesting: "cancel_validation_testing",
  getValidationCommandCandidates: "get_validation_command_candidates",
  getValidationCommandApprovalStatus: "get_validation_command_approval_status",
  approveValidationCommand: "approve_validation_command",
  getProjectRootValidationApprovalStatus: "get_project_root_validation_approval_status",
  approveProjectRootValidation: "approve_project_root_validation",
  startClaudeReview: "start_claude_review",
  cancelClaudeReview: "cancel_claude_review",
  getReviewResult: "get_review_result",
  preparePlanningContextPackage: "prepare_planning_context_package",
  prepareImplementationContextPackage: "prepare_implementation_context_package",
  prepareReviewContextPackage: "prepare_review_context_package",
  getContextPackageReviewReadiness: "get_context_package_review_readiness",
  startClaudeReviewContextPackage: "start_claude_review_context_package",
  getHighRiskApprovalStatus: "get_high_risk_approval_status",
  approveHighRiskOperation: "approve_high_risk_operation",
  getUserDiffForReview: "get_user_diff_for_review",
  approveUserDiff: "approve_user_diff",
  approveUserDiffAndStartMerge: "approve_user_diff_and_start_merge",
  confirmManualResolutionAndStartMergeContinue: "confirm_manual_resolution_and_start_merge_continue",
  confirmMergeAbortAndStart: "confirm_merge_abort_and_start",
} as const;

export type InvokeTransport = (
  command: string,
  payload?: Record<string, unknown>,
) => Promise<unknown>;

export interface IpcClient {
  getVersion(): Promise<VersionDto>;
  getHealth(): Promise<HealthDto>;
  getSystemStatus(): Promise<SystemStatusDto>;
  getBootstrapStatus(): Promise<BootstrapStatusDto>;
  getLegacyMigrationDiagnostic(): Promise<LegacyMigrationDiagnosticDto | null>;
  listProjects(): Promise<ProjectDto[]>;
  inspectProjectCandidate(inputPath: string): Promise<ProjectCandidateDto>;
  registerProject(inputPath: string, confirmationToken: string, name?: string): Promise<ProjectDto>;
  getProjectGitStatus(projectId: string): Promise<ProjectStatusDto>;
  createIsolationTask(projectId: string, brief: TaskBriefInput): Promise<TaskIsolationDto>;
  getTaskIsolation(taskId: string): Promise<TaskIsolationDto>;
  approveGitInitialization(taskId: string, expectedVersion: number): Promise<TaskIsolationDto>;
  createTaskWorktree(taskId: string, expectedVersion: number): Promise<TaskIsolationDto>;
  getActiveTask(): Promise<ActiveTaskDto | null>;
  getTask(taskId: string): Promise<TaskDto>;
  listTaskHistory(taskId: string): Promise<TaskTransitionDto[]>;
  getProviderEligibility(taskId: string): Promise<readonly ProviderEligibilityDto[]>;
  setClaudeExecutablePath(path: string): Promise<SetClaudeExecutablePathDto>;
  refreshClaudeCapability(): Promise<RefreshClaudeCapabilityDto>;
  startClaudePlanning(taskId: string, expectedVersion: number): Promise<TaskDto>;
  cancelClaudePlanning(taskId: string): Promise<CancelPlanningDto>;
  getPlanningResult(taskId: string): Promise<PlanningResultDto | null>;
  getPostMergeValidationResults(taskId: string): Promise<readonly PostMergeValidationResultDto[]>;
  getMergeConflictInspection(taskId: string): Promise<MergeConflictInspectionDto | null>;
  getContextPackagePlanningReadiness(
    taskId: string,
    expectedVersion: number,
  ): Promise<ContextPackagePlanningReadinessDto>;
  startClaudePlanningContextPackage(taskId: string, expectedVersion: number): Promise<TaskDto>;
  startClaudeImplementation(taskId: string, expectedVersion: number): Promise<TaskDto>;
  cancelClaudeImplementation(taskId: string): Promise<CancelImplementationDto>;
  getContextPackageImplementationReadiness(
    taskId: string,
    expectedVersion: number,
  ): Promise<ContextPackageImplementationReadinessDto>;
  startClaudeImplementationContextPackage(taskId: string, expectedVersion: number): Promise<TaskDto>;
  getValidationCommandCandidates(taskId: string): Promise<readonly ValidationCommandCandidateDto[]>;
  getValidationCommandApprovalStatus(taskId: string): Promise<ValidationCommandApprovalStatusDto>;
  approveValidationCommand(
    taskId: string,
    expectedVersion: number,
    input: ApproveValidationCommandInput,
  ): Promise<ApproveValidationCommandResultDto>;
  getProjectRootValidationApprovalStatus(
    taskId: string,
    expectedVersion: number,
  ): Promise<ProjectRootValidationApprovalStatusDto>;
  approveProjectRootValidation(
    taskId: string,
    expectedVersion: number,
    input: ApproveProjectRootValidationInput,
  ): Promise<ProjectRootValidationApprovalStatusDto>;
  startValidationTesting(taskId: string, expectedVersion: number): Promise<TaskDto>;
  cancelValidationTesting(taskId: string): Promise<CancelTestingDto>;
  startClaudeReview(taskId: string, expectedVersion: number): Promise<TaskDto>;
  cancelClaudeReview(taskId: string): Promise<CancelReviewDto>;
  getReviewResult(taskId: string): Promise<ReviewResultDto | null>;
  preparePlanningContextPackage(
    taskId: string,
    expectedVersion: number,
  ): Promise<ContextPackagePreparationDto>;
  prepareImplementationContextPackage(
    taskId: string,
    expectedVersion: number,
  ): Promise<ContextPackagePreparationDto>;
  prepareReviewContextPackage(
    taskId: string,
    expectedVersion: number,
  ): Promise<ContextPackagePreparationDto>;
  getContextPackageReviewReadiness(
    taskId: string,
    expectedVersion: number,
  ): Promise<ContextPackageReviewReadinessDto>;
  startClaudeReviewContextPackage(taskId: string, expectedVersion: number): Promise<TaskDto>;
  getHighRiskApprovalStatus(
    taskId: string,
    expectedVersion: number,
    riskCategory: HighRiskCategory,
  ): Promise<HighRiskApprovalStatusDto>;
  approveHighRiskOperation(
    taskId: string,
    expectedVersion: number,
    riskCategory: HighRiskCategory,
  ): Promise<HighRiskApprovalDto>;
  getUserDiffForReview(taskId: string, expectedVersion: number): Promise<RawUserDiffForReviewDto>;
  approveUserDiff(
    taskId: string,
    expectedVersion: number,
    expectedDiffContentHash: string,
  ): Promise<UserDiffApprovalDto>;
  approveUserDiffAndStartMerge(
    taskId: string,
    expectedVersion: number,
    expectedDiffContentHash: string,
  ): Promise<TaskDto>;
  confirmManualResolutionAndStartMergeContinue(taskId: string, expectedVersion: number): Promise<TaskDto>;
  confirmMergeAbortAndStart(taskId: string, expectedVersion: number): Promise<MergeAbortStartDto>;
}

const tauriTransport: InvokeTransport = (command, payload) =>
  invoke<unknown>(command, payload);

export function createIpcClient(transport: InvokeTransport = tauriTransport): IpcClient {
  const request = async <T>(
    command: string,
    guard: (value: unknown) => value is T,
    payload?: Record<string, unknown>,
  ): Promise<T> => {
    try {
      const result = await transport(command, payload);
      if (!guard(result)) {
        throw new FrontendError({
          code: "IPC_INVALID_RESPONSE",
          message: "The application returned an invalid response.",
          severity: "error",
          retry: "never",
        });
      }
      return result;
    } catch (error: unknown) {
      throw toFrontendError(error);
    }
  };

  return {
    getVersion: () => request(IPC_COMMANDS.getVersion, isVersionDto),
    getHealth: () => request(IPC_COMMANDS.getHealth, isHealthDto),
    getSystemStatus: () => request(IPC_COMMANDS.getSystemStatus, isSystemStatusDto),
    getBootstrapStatus: () => request(IPC_COMMANDS.getBootstrapStatus, isBootstrapStatusDto),
    getLegacyMigrationDiagnostic: () =>
      request(
        IPC_COMMANDS.getLegacyMigrationDiagnostic,
        (value): value is LegacyMigrationDiagnosticDto | null =>
          value === null || isLegacyMigrationDiagnosticDto(value),
      ),
    listProjects: () => request(IPC_COMMANDS.listProjects, isProjectDtoArray),
    inspectProjectCandidate: (inputPath) => request(IPC_COMMANDS.inspectProjectCandidate, isProjectCandidateDto, { inputPath }),
    registerProject: (inputPath, confirmationToken, name) => request(IPC_COMMANDS.registerProject, isProjectDto, { inputPath, confirmationToken, name: name ?? null }),
    getProjectGitStatus: (projectId) => request(IPC_COMMANDS.getProjectGitStatus, isProjectStatusDto, { projectId }),
    createIsolationTask: (projectId, brief) => request(IPC_COMMANDS.createIsolationTask, isTaskIsolationDto, { projectId, brief }),
    getTaskIsolation: (taskId) => request(IPC_COMMANDS.getTaskIsolation, isTaskIsolationDto, { taskId }),
    approveGitInitialization: (taskId, expectedVersion) => request(IPC_COMMANDS.approveGitInitialization, isTaskIsolationDto, { taskId, expectedVersion }),
    createTaskWorktree: (taskId, expectedVersion) => request(IPC_COMMANDS.createTaskWorktree, isTaskIsolationDto, { taskId, expectedVersion }),
    getActiveTask: () => request(IPC_COMMANDS.getActiveTask, isNullableActiveTaskDto),
    getTask: (taskId) => request(IPC_COMMANDS.getTask, isTaskDto, { taskId }),
    listTaskHistory: (taskId) =>
      request(IPC_COMMANDS.listTaskHistory, isTaskTransitionDtoArray, { taskId }),
    getProviderEligibility: (taskId) =>
      request(IPC_COMMANDS.getProviderEligibility, isProviderEligibilityDtoArray, { taskId }),
    setClaudeExecutablePath: (path) =>
      request(IPC_COMMANDS.setClaudeExecutablePath, isSetClaudeExecutablePathDto, { path }),
    refreshClaudeCapability: () =>
      request(IPC_COMMANDS.refreshClaudeCapability, isRefreshClaudeCapabilityDto),
    startClaudePlanning: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startClaudePlanning, isTaskDto, { taskId, expectedVersion }),
    cancelClaudePlanning: (taskId) =>
      request(IPC_COMMANDS.cancelClaudePlanning, isCancelPlanningDto, { taskId }),
    getPlanningResult: (taskId) =>
      request(IPC_COMMANDS.getPlanningResult, isNullablePlanningResultDto, { taskId }),
    getPostMergeValidationResults: (taskId) =>
      request(IPC_COMMANDS.getPostMergeValidationResults, isPostMergeValidationResultDtoArray, { taskId }),
    getMergeConflictInspection: (taskId) =>
      request(IPC_COMMANDS.getMergeConflictInspection, isNullableMergeConflictInspectionDto, { taskId }),
    getContextPackagePlanningReadiness: (taskId, expectedVersion) =>
      request(
        IPC_COMMANDS.getContextPackagePlanningReadiness,
        isContextPackagePlanningReadinessDto,
        { taskId, expectedVersion },
      ),
    startClaudePlanningContextPackage: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startClaudePlanningContextPackage, isTaskDto, {
        taskId,
        expectedVersion,
      }),
    startClaudeImplementation: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startClaudeImplementation, isTaskDto, { taskId, expectedVersion }),
    cancelClaudeImplementation: (taskId) =>
      request(IPC_COMMANDS.cancelClaudeImplementation, isCancelImplementationDto, { taskId }),
    getContextPackageImplementationReadiness: (taskId, expectedVersion) =>
      request(
        IPC_COMMANDS.getContextPackageImplementationReadiness,
        isContextPackageImplementationReadinessDto,
        { taskId, expectedVersion },
      ),
    startClaudeImplementationContextPackage: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startClaudeImplementationContextPackage, isTaskDto, {
        taskId,
        expectedVersion,
      }),
    getValidationCommandCandidates: (taskId) =>
      request(IPC_COMMANDS.getValidationCommandCandidates, isValidationCommandCandidateDtoArray, {
        taskId,
      }),
    getValidationCommandApprovalStatus: (taskId) =>
      request(
        IPC_COMMANDS.getValidationCommandApprovalStatus,
        isValidationCommandApprovalStatusDto,
        { taskId },
      ),
    approveValidationCommand: (taskId, expectedVersion, input) =>
      request(IPC_COMMANDS.approveValidationCommand, isApproveValidationCommandResultDto, {
        taskId,
        expectedVersion,
        input,
      }),
    getProjectRootValidationApprovalStatus: (taskId, expectedVersion) =>
      request(
        IPC_COMMANDS.getProjectRootValidationApprovalStatus,
        isProjectRootValidationApprovalStatusDto,
        { taskId, expectedVersion },
      ),
    approveProjectRootValidation: (taskId, expectedVersion, input) =>
      request(
        IPC_COMMANDS.approveProjectRootValidation,
        isProjectRootValidationApprovalStatusDto,
        { taskId, expectedVersion, input },
      ),
    startValidationTesting: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startValidationTesting, isTaskDto, { taskId, expectedVersion }),
    cancelValidationTesting: (taskId) =>
      request(IPC_COMMANDS.cancelValidationTesting, isCancelTestingDto, { taskId }),
    startClaudeReview: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startClaudeReview, isTaskDto, { taskId, expectedVersion }),
    cancelClaudeReview: (taskId) =>
      request(IPC_COMMANDS.cancelClaudeReview, isCancelReviewDto, { taskId }),
    getReviewResult: (taskId) =>
      request(IPC_COMMANDS.getReviewResult, isNullableReviewResultDto, { taskId }),
    preparePlanningContextPackage: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.preparePlanningContextPackage, isContextPackagePreparationDto, {
        taskId,
        expectedVersion,
      }),
    prepareImplementationContextPackage: (taskId, expectedVersion) =>
      request(
        IPC_COMMANDS.prepareImplementationContextPackage,
        isContextPackagePreparationDto,
        { taskId, expectedVersion },
      ),
    prepareReviewContextPackage: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.prepareReviewContextPackage, isContextPackagePreparationDto, {
        taskId,
        expectedVersion,
      }),
    getContextPackageReviewReadiness: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.getContextPackageReviewReadiness, isContextPackageReviewReadinessDto, {
        taskId,
        expectedVersion,
      }),
    startClaudeReviewContextPackage: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.startClaudeReviewContextPackage, isTaskDto, {
        taskId,
        expectedVersion,
      }),
    getHighRiskApprovalStatus: (taskId, expectedVersion, riskCategory) =>
      request(IPC_COMMANDS.getHighRiskApprovalStatus, isHighRiskApprovalStatusDto, {
        taskId,
        expectedVersion,
        riskCategory,
      }),
    approveHighRiskOperation: (taskId, expectedVersion, riskCategory) =>
      request(IPC_COMMANDS.approveHighRiskOperation, isHighRiskApprovalDto, {
        taskId,
        expectedVersion,
        riskCategory,
      }),
    getUserDiffForReview: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.getUserDiffForReview, isRawUserDiffForReviewDto, {
        taskId,
        expectedVersion,
      }),
    approveUserDiff: (taskId, expectedVersion, expectedDiffContentHash) =>
      request(IPC_COMMANDS.approveUserDiff, isUserDiffApprovalDto, {
        taskId,
        expectedVersion,
        expectedDiffContentHash,
      }),
    approveUserDiffAndStartMerge: (taskId, expectedVersion, expectedDiffContentHash) =>
      request(IPC_COMMANDS.approveUserDiffAndStartMerge, isTaskDto, {
        taskId,
        expectedVersion,
        expectedDiffContentHash,
      }),
    confirmManualResolutionAndStartMergeContinue: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.confirmManualResolutionAndStartMergeContinue, isExactTaskDto, {
        taskId,
        expectedVersion,
      }),
    confirmMergeAbortAndStart: (taskId, expectedVersion) =>
      request(IPC_COMMANDS.confirmMergeAbortAndStart, isMergeAbortStartDto, {
        taskId,
        expectedVersion,
      }),
  };
}

export const ipcClient = createIpcClient();

const HEALTH_STATES: readonly HealthState[] = ["healthy", "degraded", "unavailable"];
const STORAGE_STATUSES: readonly StorageStatus[] = [
  "ready",
  "unavailable",
  "insecure",
  "unsupported",
];
const DATABASE_STATUSES: readonly DatabaseStatus[] = [
  "notChecked",
  "ready",
  "upgraded",
  "migrationRequired",
  "unavailable",
  "incompatible",
];
const LOGGING_STATUSES: readonly LoggingStatus[] = ["notChecked", "ready", "unavailable"];
const CAPABILITY_STATUSES = ["supported", "unsupported", "unavailable"] as const;
const ACTIVE_STATUSES = ["notChecked", "none", "active"] as const;
const TASK_STATES: readonly TaskState[] = [
  "created",
  "projectValidated",
  "awaitingGitInitApproval",
  "gitInitialized",
  "worktreeCreating",
  "worktreeReady",
  "planning",
  "awaitingDesignApproval",
  "implementing",
  "testing",
  "autoFixing",
  "reviewing",
  "reviewFixing",
  "awaitingUserDiffApproval",
  "merging",
  "mergeConflict",
  "postMergeTesting",
  "completed",
  "paused",
  "failed",
  "recoveryRequired",
  "unknownExternalEffect",
  "cancelled",
  "cleanupPending",
  "archived",
];
const REPOSITORY_KINDS = ["git", "nonGit"] as const;
const ISOLATION_STATUSES = ["awaitingGitInitApproval", "ready", "gitInitInProgress", "worktreeCreating", "worktreeReady", "recoveryRequired"] as const;
const ISOLATION_BLOCKERS = ["dirtyRepository", "detachedHead", "unbornRepository", "missingCurrentBranch", "gitAuthorMissing", "gitOperationFailed", "recoveryRequired"] as const;
const REFRESH_OUTCOMES: readonly RefreshOutcome[] = ["completed", "superseded", "conflict"];

function isVersionDto(value: unknown): value is VersionDto {
  return isRecord(value) && typeof value.version === "string";
}

function isHealthDto(value: unknown): value is HealthDto {
  return isRecord(value) && isOneOf(value.status, HEALTH_STATES);
}

function isActiveTaskStatusDto(value: unknown): value is ActiveTaskStatusDto {
  return (
    isRecord(value) &&
    isOneOf(value.status, ACTIVE_STATUSES) &&
    isNullableString(value.taskId) &&
    isNullableNumber(value.acquiredAtMs)
  );
}

function isBootstrapStatusDto(value: unknown): value is BootstrapStatusDto {
  return (
    isRecord(value) &&
    isOneOf(value.storageStatus, STORAGE_STATUSES) &&
    isOneOf(value.databaseStatus, DATABASE_STATUSES) &&
    isOneOf(value.loggingStatus, LOGGING_STATUSES) &&
    isActiveTaskStatusDto(value.activeTaskStatus) &&
    typeof value.applicationVersion === "string" &&
    typeof value.ready === "boolean"
  );
}

function isLegacyMigrationDiagnosticDto(
  value: unknown,
): value is LegacyMigrationDiagnosticDto {
  return (
    isRecord(value) &&
    typeof value.projectId === "string" &&
    typeof value.displayPath === "string" &&
    typeof value.reasonCode === "string"
  );
}

function isCapabilityDto(value: unknown): value is CapabilityDto {
  return (
    isRecord(value) &&
    isOneOf(value.secureStorage, CAPABILITY_STATUSES) &&
    isOneOf(value.nativePermissions, CAPABILITY_STATUSES) &&
    isOneOf(value.gitExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.claudeExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.codexExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.updater, CAPABILITY_STATUSES) &&
    isOneOf(value.installerManagement, CAPABILITY_STATUSES)
  );
}

function isSystemStatusDto(value: unknown): value is SystemStatusDto {
  return (
    isRecord(value) &&
    typeof value.applicationVersion === "string" &&
    isOneOf(value.health, HEALTH_STATES) &&
    isOneOf(value.storageStatus, STORAGE_STATUSES) &&
    isOneOf(value.databaseStatus, DATABASE_STATUSES) &&
    isOneOf(value.loggingStatus, LOGGING_STATUSES) &&
    isActiveTaskStatusDto(value.activeTaskStatus) &&
    isCapabilityDto(value.capabilities)
  );
}

function isProjectDto(value: unknown): value is ProjectDto {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.name === "string" &&
    typeof value.displayPath === "string" &&
    typeof value.createdAtMs === "number" &&
    typeof value.updatedAtMs === "number"
  );
}

function isRepositoryStatusDto(value: unknown): value is import("./types").RepositoryStatusDto {
  return isRecord(value) && typeof value.clean === "boolean" && typeof value.detachedHead === "boolean" && isNullableString(value.currentBranch) && isNullableString(value.headCommit);
}
function isProjectCandidateDto(value: unknown): value is ProjectCandidateDto {
  return isRecord(value) && typeof value.suggestedName === "string" && typeof value.displayPath === "string" && typeof value.confirmationToken === "string" && isOneOf(value.repositoryKind, REPOSITORY_KINDS) && (value.repositoryStatus === null || isRepositoryStatusDto(value.repositoryStatus));
}
function isProjectStatusDto(value: unknown): value is ProjectStatusDto {
  return isRecord(value) && typeof value.projectId === "string" && isOneOf(value.repositoryKind, REPOSITORY_KINDS) && (value.repositoryStatus === null || isRepositoryStatusDto(value.repositoryStatus));
}
function isTaskIsolationDto(value: unknown): value is TaskIsolationDto {
  return isRecord(value) && typeof value.taskId === "string" && typeof value.projectId === "string" && isOneOf(value.taskState, TASK_STATES) && typeof value.taskVersion === "number" && isOneOf(value.isolationStatus, ISOLATION_STATUSES) && typeof value.branchIdentity === "string" && isNullableString(value.baseBranch) && isNullableString(value.baseCommit) && (value.blocker === null || isOneOf(value.blocker, ISOLATION_BLOCKERS));
}

function isProjectDtoArray(value: unknown): value is ProjectDto[] {
  return Array.isArray(value) && value.every(isProjectDto);
}

function isActiveTaskDto(value: unknown): value is ActiveTaskDto {
  return isRecord(value) && typeof value.taskId === "string" && typeof value.acquiredAtMs === "number";
}

function isNullableActiveTaskDto(value: unknown): value is ActiveTaskDto | null {
  return value === null || isActiveTaskDto(value);
}

function isTaskBriefDto(value: unknown): value is TaskBriefDto {
  return (
    isRecord(value) &&
    typeof value.requirements === "string" &&
    typeof value.completionCriteria === "string" &&
    typeof value.prohibitedScope === "string" &&
    typeof value.createdAtMs === "number"
  );
}

function isTaskDto(value: unknown): value is TaskDto {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.projectId === "string" &&
    isOneOf(value.state, TASK_STATES) &&
    typeof value.version === "number" &&
    typeof value.branchIdentity === "string" &&
    (value.resumeTargetState === null || isOneOf(value.resumeTargetState, TASK_STATES)) &&
    typeof value.createdAtMs === "number" &&
    typeof value.updatedAtMs === "number" &&
    isNullableNumber(value.terminalAtMs) &&
    (value.brief === null || isTaskBriefDto(value.brief))
  );
}

const TASK_DTO_KEYS = [
  "id",
  "projectId",
  "state",
  "version",
  "branchIdentity",
  "resumeTargetState",
  "createdAtMs",
  "updatedAtMs",
  "terminalAtMs",
  "brief",
] as const;

function hasExactTaskDtoKeys(value: Record<string, unknown>): boolean {
  const keys = Object.keys(value);
  const allowed: readonly string[] = TASK_DTO_KEYS;
  return keys.length === allowed.length && keys.every((key) => allowed.includes(key));
}

// Stricter than `isTaskDto`: also rejects any key beyond backend `TaskDto`'s
// exact serialized key set (e.g. a resolution digest, raw path, or Git
// stdout/stderr accidentally attached to a future response). Used only for
// `confirmManualResolutionAndStartMergeContinue`, whose merge-continue write
// must never let such content reach this response even if the backend
// changes later -- `isTaskDto` itself stays loose for every other command
// that already relies on it.
function isExactTaskDto(value: unknown): value is TaskDto {
  return isRecord(value) && hasExactTaskDtoKeys(value) && isTaskDto(value);
}

function isTaskTransitionDto(value: unknown): value is TaskTransitionDto {
  return (
    isRecord(value) &&
    typeof value.sequence === "number" &&
    (value.fromState === null || isOneOf(value.fromState, TASK_STATES)) &&
    isOneOf(value.toState, TASK_STATES) &&
    typeof value.taskVersion === "number" &&
    typeof value.occurredAtMs === "number"
  );
}

function isTaskTransitionDtoArray(value: unknown): value is TaskTransitionDto[] {
  return Array.isArray(value) && value.every(isTaskTransitionDto);
}

function isSetClaudeExecutablePathDto(
  value: unknown,
): value is SetClaudeExecutablePathDto {
  return (
    isRecord(value) &&
    typeof value.displayPath === "string" &&
    isOneOf(value.claudeExecution, CAPABILITY_STATUSES)
  );
}

function isCancelPlanningDto(value: unknown): value is CancelPlanningDto {
  return isRecord(value) && typeof value.requested === "boolean";
}

function isCancelImplementationDto(value: unknown): value is CancelImplementationDto {
  return isRecord(value) && typeof value.requested === "boolean";
}

function isRefreshClaudeCapabilityDto(
  value: unknown,
): value is RefreshClaudeCapabilityDto {
  return (
    isRecord(value) &&
    isOneOf(value.outcome, REFRESH_OUTCOMES) &&
    isOneOf(value.claudeExecution, CAPABILITY_STATUSES) &&
    isOneOf(value.codexExecution, CAPABILITY_STATUSES)
  );
}

function isOneOf<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && allowed.includes(value as T);
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isNullableNumber(value: unknown): value is number | null {
  return value === null || typeof value === "number";
}
