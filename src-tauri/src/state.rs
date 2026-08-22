use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex, MutexGuard, RwLock,
    atomic::{AtomicU64, Ordering},
};

use chatoms_application::system::CapabilityStatus as AppCapabilityStatus;
use chatoms_application::{bootstrap::BootstrapStatus, error::ApplicationError};
use chatoms_domain::{
    ContextDataScope, HighRiskCategory, OperationRiskKind, ProjectId, Task, TaskId,
    TaskStateTransition, ValidationCommandKind, ValidationExecutionScope, WorkKind,
};
use chatoms_infrastructure::bootstrap::{
    LegacyMigrationDiagnostic, SharedDatabase, SharedFoundationRepository, SharedLoggingGuard,
    SharedResolvedAppPaths,
};
use chatoms_ports::process::AtomicCancellationSignal;
#[cfg(windows)]
pub type PreflightDirectory = chatoms_platform::preflight::TrustedPreflightWorkingDirectory;

#[cfg(not(windows))]
#[derive(Clone, Debug)]
pub struct PreflightDirectory;

#[cfg(not(windows))]
impl PreflightDirectory {
    pub fn revalidate(&self) -> Result<(), chatoms_ports::error::PortFailure> {
        Err(chatoms_ports::error::PortFailure::new(
            chatoms_ports::error::FailureCategory::Unsupported,
        ))
    }

    pub fn path(&self) -> &std::path::Path {
        std::path::Path::new(".")
    }
}
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, TimeProvider,
    diff::DiffContentHash,
    error::PortFailure,
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome, WorktreePathProvider,
    },
    manual_merge_resolution::ManualResolutionDigest,
    provider::ProviderKind,
    repository::{
        ActiveLease, AppProfileRecord, ContextPackageManifestRecord, ContextPackagePreparation,
        DiffApprovalRecord, FoundationRepository, GitInitApproval, GitOperationAttempt,
        GitOperationReceipt, GitOperationReceiptKind, HighRiskApprovalRecord,
        ManualMergeResolutionConfirmationRecord, MergeAbortApprovalRecord,
        OperationRiskDeclaration, OperationRiskDeclarationRecord, PostMergeValidationResultAttempt,
        PostMergeValidationResultRecord, ProjectFilesystemIdentityRecord, ProjectRecord,
        ProjectSummary, ProviderBindingRecord, ProviderConsent, RepositoryError, TaskBriefRecord,
        TaskGitIsolation, TaskImplementationResultRecord, TaskPlanningResultRecord,
        TaskReviewResultRecord, ValidationCommandApprovalRecord, ValidationCommandResultAttempt,
        ValidationCommandResultRecord,
    },
};

use crate::error::IpcErrorDto;

#[derive(Clone)]
pub struct RepositoryHandle {
    inner: Arc<Mutex<Box<dyn FoundationRepository + Send>>>,
}

impl RepositoryHandle {
    pub fn new(repository: impl FoundationRepository + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(repository))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn FoundationRepository) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RepositoryError::new(chatoms_ports::repository::RepositoryErrorCode::OperationFailed)
        })?;
        operation(inner.as_mut())
    }
}

impl FoundationRepository for RepositoryHandle {
    fn create_project(&mut self, project: &ProjectRecord) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.create_project(project))
    }

    fn create_project_with_identity(
        &mut self,
        project: &ProjectRecord,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.create_project_with_identity(project, identity))
    }

    fn get_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_project_identity(project_id))
    }

    fn update_project_identity(
        &mut self,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.update_project_identity(identity))
    }

    fn get_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_project(project_id))
    }

    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.create_task(task, initial_transition, lease_acquired_at_ms))
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.with_inner(|inner| inner.get_task(task_id))
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_transition(expected_version, task, transition))
    }

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_recovery_target(expected_version, task))
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.terminate_task(expected_version, task, transition))
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        self.with_inner(|inner| inner.list_task_transitions(task_id))
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.with_inner(|inner| inner.list_projects())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.with_inner(|inner| inner.active_lease())
    }

    fn create_isolation_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        classified_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
        isolation: &TaskGitIsolation,
        brief: Option<&TaskBriefRecord>,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.create_isolation_task(
                task,
                initial_transition,
                classified_transition,
                lease_acquired_at_ms,
                isolation,
                brief,
            )
        })
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_isolation(task_id))
    }

    fn get_task_brief(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskBriefRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_brief(task_id))
    }

    fn get_task_planning_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<chatoms_ports::repository::TaskPlanningResultRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_planning_result(task_id))
    }

    fn get_task_implementation_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskImplementationResultRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_implementation_result(task_id))
    }

    fn get_task_review_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskReviewResultRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_task_review_result(task_id))
    }

    fn get_provider_consent(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_provider_consent(
                task_id,
                provider,
                work_kind,
                approved_task_version,
                data_scope,
            )
        })
    }

    fn save_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_planning_transition(expected_version, task, transition, consent)
        })
    }

    fn save_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_implementation_transition(expected_version, task, transition, consent)
        })
    }

    fn save_review_consent(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        data_scope: ContextDataScope,
        consented_at_ms: i64,
    ) -> Result<ProviderConsent, RepositoryError> {
        self.with_inner(|inner| {
            inner.save_review_consent(expected_version, task_id, data_scope, consented_at_ms)
        })
    }

    fn save_context_package_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_context_package_planning_transition(expected_version, task, transition)
        })
    }

    fn save_context_package_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_context_package_implementation_transition(expected_version, task, transition)
        })
    }

    fn prepare_planning_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.with_inner(|inner| {
            inner.prepare_planning_context_package(expected_version, task_id, prepared_at_ms)
        })
    }

    fn prepare_implementation_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.with_inner(|inner| {
            inner.prepare_implementation_context_package(expected_version, task_id, prepared_at_ms)
        })
    }

    fn prepare_review_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.with_inner(|inner| {
            inner.prepare_review_context_package(expected_version, task_id, prepared_at_ms)
        })
    }

    fn save_context_package_manifest(
        &mut self,
        record: &ContextPackageManifestRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_context_package_manifest(record))
    }

    fn get_context_package_manifest(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ContextPackageManifestRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_context_package_manifest(
                task_id,
                provider,
                work_kind,
                approved_task_version,
                data_scope,
            )
        })
    }

    fn save_high_risk_approval(
        &mut self,
        approval: &HighRiskApprovalRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_high_risk_approval(approval))
    }

    fn get_high_risk_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_high_risk_approval(task_id, approved_task_version, risk_category)
        })
    }

    fn ensure_high_risk_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
        approved_at_ms: i64,
    ) -> Result<HighRiskApprovalRecord, RepositoryError> {
        self.with_inner(|inner| {
            inner.ensure_high_risk_approval(
                task_id,
                expected_version,
                risk_category,
                approved_at_ms,
            )
        })
    }

    fn declare_operation_risk(
        &mut self,
        declaration: &OperationRiskDeclarationRecord,
        risk_categories: &[HighRiskCategory],
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.declare_operation_risk(declaration, risk_categories))
    }

    fn get_operation_risk_declaration(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        operation_kind: OperationRiskKind,
    ) -> Result<Option<OperationRiskDeclaration>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_operation_risk_declaration(task_id, approved_task_version, operation_kind)
        })
    }

    fn save_diff_approval(&mut self, approval: &DiffApprovalRecord) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_diff_approval(approval))
    }

    fn get_diff_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        diff_content_hash: DiffContentHash,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_diff_approval(task_id, approved_task_version, diff_content_hash)
        })
    }

    fn get_diff_approval_for_task_version(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_diff_approval_for_task_version(task_id, approved_task_version)
        })
    }

    fn ensure_diff_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        diff_content_hash: DiffContentHash,
        approved_at_ms: i64,
    ) -> Result<DiffApprovalRecord, RepositoryError> {
        self.with_inner(|inner| {
            inner.ensure_diff_approval(task_id, expected_version, diff_content_hash, approved_at_ms)
        })
    }

    fn get_manual_merge_resolution_confirmation(
        &mut self,
        task_id: TaskId,
        merge_conflict_task_version: u64,
        resolution_digest: ManualResolutionDigest,
    ) -> Result<Option<ManualMergeResolutionConfirmationRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_manual_merge_resolution_confirmation(
                task_id,
                merge_conflict_task_version,
                resolution_digest,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_manual_merge_resolution_confirmation(
        &mut self,
        task_id: TaskId,
        merge_conflict_task_version: u64,
        source_approval_task_version: u64,
        base_commit: &str,
        task_commit: &str,
        merge_head_commit: &str,
        resolution_digest: ManualResolutionDigest,
        confirmed_at_ms: i64,
    ) -> Result<ManualMergeResolutionConfirmationRecord, RepositoryError> {
        self.with_inner(|inner| {
            inner.ensure_manual_merge_resolution_confirmation(
                task_id,
                merge_conflict_task_version,
                source_approval_task_version,
                base_commit,
                task_commit,
                merge_head_commit,
                resolution_digest,
                confirmed_at_ms,
            )
        })
    }

    fn save_manual_merge_resolution_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        resolution_digest: ManualResolutionDigest,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_manual_merge_resolution_transition(
                expected_version,
                task,
                transition,
                resolution_digest,
            )
        })
    }

    fn get_merge_abort_approval(
        &mut self,
        task_id: TaskId,
        merge_conflict_task_version: u64,
    ) -> Result<Option<MergeAbortApprovalRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.get_merge_abort_approval(task_id, merge_conflict_task_version)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_merge_abort_approval(
        &mut self,
        task_id: TaskId,
        merge_conflict_task_version: u64,
        source_approval_task_version: u64,
        base_commit: &str,
        task_commit: &str,
        merge_head_commit: &str,
        approved_at_ms: i64,
    ) -> Result<MergeAbortApprovalRecord, RepositoryError> {
        self.with_inner(|inner| {
            inner.ensure_merge_abort_approval(
                task_id,
                merge_conflict_task_version,
                source_approval_task_version,
                base_commit,
                task_commit,
                merge_head_commit,
                approved_at_ms,
            )
        })
    }

    fn save_merge_abort_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_merge_abort_transition(expected_version, task, transition, terminal)
        })
    }

    fn save_planning_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskPlanningResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_planning_result(expected_version, task, transition, result, terminal)
        })
    }

    fn save_implementation_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskImplementationResultRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_implementation_result(expected_version, task, transition, result)
        })
    }

    fn save_review_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskReviewResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_review_result(expected_version, task, transition, result, terminal)
        })
    }

    fn save_validation_command_approval(
        &mut self,
        approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_validation_command_approval(approval))
    }

    fn list_validation_command_approvals(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.list_validation_command_approvals(task_id, approved_task_version)
        })
    }

    fn list_validation_command_approvals_for_scope(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        execution_scope: ValidationExecutionScope,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.list_validation_command_approvals_for_scope(
                task_id,
                approved_task_version,
                execution_scope,
            )
        })
    }

    fn append_validation_command_result(
        &mut self,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<ValidationCommandResultRecord, RepositoryError> {
        self.with_inner(|inner| inner.append_validation_command_result(attempt))
    }

    fn list_validation_command_results(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<ValidationCommandResultRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.list_validation_command_results(task_id, approved_task_version, kind)
        })
    }

    fn finalize_validation_command_batch(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.finalize_validation_command_batch(expected_version, task, transition, attempt)
        })
    }

    fn append_post_merge_validation_result(
        &mut self,
        attempt: &PostMergeValidationResultAttempt,
    ) -> Result<PostMergeValidationResultRecord, RepositoryError> {
        self.with_inner(|inner| inner.append_post_merge_validation_result(attempt))
    }

    fn list_post_merge_validation_results(
        &mut self,
        task_id: TaskId,
        approval_task_version: u64,
        post_merge_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<PostMergeValidationResultRecord>, RepositoryError> {
        self.with_inner(|inner| {
            inner.list_post_merge_validation_results(
                task_id,
                approval_task_version,
                post_merge_task_version,
                kind,
            )
        })
    }

    fn finalize_post_merge_validation_batch(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        attempt: &PostMergeValidationResultAttempt,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.finalize_post_merge_validation_batch(expected_version, task, transition, attempt)
        })
    }

    fn begin_git_initialization(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
        approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.begin_git_initialization(expected_version, isolation, approval)
        })
    }

    fn save_isolation_intent(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| inner.save_isolation_intent(expected_version, isolation))
    }

    fn append_git_operation_receipt(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.append_git_operation_receipt(operation_id, kind, evidence, recorded_at_ms)
        })
    }

    fn list_git_operation_receipts(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        self.with_inner(|inner| inner.list_git_operation_receipts(operation_id))
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        self.with_inner(|inner| inner.list_incomplete_git_operations())
    }

    fn save_isolation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_isolation_transition(expected_version, task, transition, isolation)
        })
    }

    fn save_git_initialization_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_git_initialization_completion(
                expected_version,
                task,
                transition,
                isolation,
                identity,
            )
        })
    }

    fn save_worktree_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.save_worktree_completion(expected_version, task, transition, isolation)
        })
    }

    fn terminate_isolation_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.terminate_isolation_task(expected_version, task, transition, isolation)
        })
    }

    fn ensure_default_profile_and_claude_binding(
        &mut self,
        profile: &AppProfileRecord,
        binding: &ProviderBindingRecord,
    ) -> Result<ProviderBindingRecord, RepositoryError> {
        self.with_inner(|inner| inner.ensure_default_profile_and_claude_binding(profile, binding))
    }

    fn get_claude_binding(
        &mut self,
        profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        self.with_inner(|inner| inner.get_claude_binding(profile_name))
    }

    fn update_claude_executable_path(
        &mut self,
        binding_id: &str,
        executable_path: Option<&str>,
        updated_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_inner(|inner| {
            inner.update_claude_executable_path(binding_id, executable_path, updated_at_ms)
        })
    }
}

#[derive(Clone)]
pub struct GitServiceHandle {
    inner: Arc<Mutex<Box<dyn GitService + Send>>>,
}

impl GitServiceHandle {
    pub fn new(service: impl GitService + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(service))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn GitService) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl GitService for GitServiceHandle {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.with_inner(|inner| inner.is_available())
    }
    fn inspect_project(
        &mut self,
        input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        self.with_inner(|inner| inner.inspect_project(input))
    }
    fn repository_status(
        &mut self,
        root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        self.with_inner(|inner| inner.repository_status(root))
    }
    fn validate_non_git_source(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.with_inner(|inner| inner.validate_non_git_source(root))
    }
    fn validate_repository_source(
        &mut self,
        root: &std::path::Path,
        base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        self.with_inner(|inner| inner.validate_repository_source(root, base_commit))
    }
    fn initialize_repository(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.with_inner(|inner| inner.initialize_repository(root))
    }
    fn has_commit_author(&mut self, root: &std::path::Path) -> Result<bool, PortFailure> {
        self.with_inner(|inner| inner.has_commit_author(root))
    }
    fn create_initial_snapshot(&mut self, root: &std::path::Path) -> Result<String, PortFailure> {
        self.with_inner(|inner| inner.create_initial_snapshot(root))
    }
    fn create_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
        safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        self.with_inner(|inner| {
            inner.create_task_worktree(root, branch, base_commit, worktree, safety)
        })
    }
    fn verify_task_worktree(
        &mut self,
        root: &std::path::Path,
        branch: &str,
        base_commit: &str,
        worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        self.with_inner(|inner| inner.verify_task_worktree(root, branch, base_commit, worktree))
    }
}

#[derive(Clone)]
pub struct WorktreePathHandle {
    inner: Arc<Mutex<Box<dyn WorktreePathProvider + Send>>>,
}

#[derive(Clone)]
pub struct FilesystemIdentityHandle {
    inner: Arc<Mutex<Box<dyn FilesystemIdentityPort + Send>>>,
}

impl FilesystemIdentityHandle {
    pub fn new(port: impl FilesystemIdentityPort + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(port))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn FilesystemIdentityPort) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl FilesystemIdentityPort for FilesystemIdentityHandle {
    fn inspect_supported_directory(
        &mut self,
        path: &std::path::Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.with_inner(|inner| inner.inspect_supported_directory(path))
    }

    fn verify_local_tree(&mut self, root: &std::path::Path) -> Result<(), PortFailure> {
        self.with_inner(|inner| inner.verify_local_tree(root))
    }

    fn acquire_guard(
        &mut self,
        path: &std::path::Path,
        expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        self.with_inner(|inner| inner.acquire_guard(path, expected))
    }

    fn inspect_supported_file(
        &mut self,
        path: &std::path::Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.with_inner(|inner| inner.inspect_supported_file(path))
    }
}

impl WorktreePathHandle {
    pub fn new(provider: impl WorktreePathProvider + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(provider))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn WorktreePathProvider) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl WorktreePathProvider for WorktreePathHandle {
    fn prepare_worktree_path(
        &mut self,
        project_id: ProjectId,
        task_id: TaskId,
    ) -> Result<std::path::PathBuf, PortFailure> {
        self.with_inner(|inner| inner.prepare_worktree_path(project_id, task_id))
    }
}

#[derive(Clone)]
pub struct TimeProviderHandle {
    inner: Arc<Mutex<Box<dyn TimeProvider + Send>>>,
}

impl TimeProviderHandle {
    pub fn new(provider: impl TimeProvider + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(provider))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn TimeProvider) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl TimeProvider for TimeProviderHandle {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        self.with_inner(|inner| inner.now_ms())
    }
}

#[derive(Clone)]
pub struct CapabilityHandle {
    inner: Arc<Mutex<Box<dyn PlatformCapabilityPort + Send>>>,
}

impl CapabilityHandle {
    pub fn new(adapter: impl PlatformCapabilityPort + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(adapter))),
        }
    }

    fn with_inner<T>(
        &self,
        operation: impl FnOnce(&mut dyn PlatformCapabilityPort) -> Result<T, PortFailure>,
    ) -> Result<T, PortFailure> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PortFailure::new(chatoms_ports::error::FailureCategory::Internal))?;
        operation(inner.as_mut())
    }
}

impl PlatformCapabilityPort for CapabilityHandle {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        self.with_inner(|inner| inner.platform_capabilities())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CachedProviderCapabilities {
    pub claude: Option<AppCapabilityStatus>,
    pub codex: Option<AppCapabilityStatus>,
}

impl CachedProviderCapabilities {
    const NOT_YET_PROBED: Self = Self {
        claude: None,
        codex: None,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    Completed,
    Superseded,
    Conflict,
}

#[derive(Clone)]
pub struct ProviderCapabilityHandle {
    generation: Arc<AtomicU64>,
    cache: Arc<RwLock<CachedProviderCapabilities>>,
    refreshing: Arc<Mutex<bool>>,
}

impl Default for ProviderCapabilityHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderCapabilityHandle {
    #[must_use]
    pub fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            cache: Arc::new(RwLock::new(CachedProviderCapabilities::NOT_YET_PROBED)),
            refreshing: Arc::new(Mutex::new(false)),
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn read_cache(&self) -> CachedProviderCapabilities {
        self.cache
            .read()
            .map(|guard| *guard)
            .unwrap_or(CachedProviderCapabilities::NOT_YET_PROBED)
    }

    pub fn invalidate_and_bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut cache) = self.cache.write() {
            *cache = CachedProviderCapabilities::NOT_YET_PROBED;
        }
    }

    pub fn try_begin_refresh(&self) -> Option<u64> {
        let mut refreshing = self.refreshing.lock().ok()?;
        if *refreshing {
            return None;
        }
        *refreshing = true;
        Some(self.generation.load(Ordering::Acquire))
    }

    pub fn finish_refresh(
        &self,
        captured_generation: u64,
        capabilities: CachedProviderCapabilities,
    ) -> RefreshOutcome {
        let result = {
            let current_generation = self.generation.load(Ordering::Acquire);
            if current_generation != captured_generation {
                RefreshOutcome::Superseded
            } else if let Ok(mut cache) = self.cache.write() {
                *cache = capabilities;
                RefreshOutcome::Completed
            } else {
                RefreshOutcome::Superseded
            }
        };
        if let Ok(mut refreshing) = self.refreshing.lock() {
            *refreshing = false;
        }
        result
    }

    pub fn abort_refresh(&self) {
        if let Ok(mut refreshing) = self.refreshing.lock() {
            *refreshing = false;
        }
    }
}

/// In-memory-only registry of cancellation handles for Claude Planning runs
/// currently executing on a background thread, keyed by task id. Never
/// persisted: an app restart has no running process to cancel anyway, and
/// `docs/PRODUCT_REQUIREMENTS.md`/AGENTS.md scope this Unit to an in-memory
/// handle, not a separate execution/session table.
#[derive(Clone, Default)]
pub struct PlanningRunRegistry {
    inner: Arc<Mutex<HashMap<TaskId, AtomicCancellationSignal>>>,
}

impl PlanningRunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh cancellation signal for `task_id`. Returns `None`
    /// instead of overwriting an existing entry: two live handles for the
    /// same task would mean two independent things could race to cancel (or
    /// fail to unregister) the same run. `TaskService::start_planning`
    /// already fails closed (via optimistic task-version concurrency) before
    /// a second Planning attempt for the same task could ever reach this
    /// call, so a `None` here indicates a caller invariant violation, not
    /// routine contention, and the caller must not silently proceed as if a
    /// handle had been registered.
    pub fn register(&self, task_id: TaskId) -> Option<AtomicCancellationSignal> {
        let signal = AtomicCancellationSignal::new();
        let mut guard = self.inner.lock().ok()?;
        if guard.contains_key(&task_id) {
            return None;
        }
        guard.insert(task_id, signal.clone());
        Some(signal)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }

    /// Requests cancellation of the in-flight run for `task_id`, if any.
    /// Returns whether a matching run was found; the caller must not infer
    /// anything about the eventual outcome from this alone — only a
    /// subsequently *confirmed* process exit is recorded as `Cancelled`.
    pub fn request_cancellation(&self, task_id: TaskId) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        let Some(signal) = guard.get(&task_id) else {
            return false;
        };
        signal.cancel();
        true
    }
}

/// In-memory-only registry of cancellation handles for Claude Implementation
/// runs currently executing on a background thread, keyed by task id.
/// Mirrors [`PlanningRunRegistry`] exactly (same shape, same invariants) but
/// is kept as a separate type rather than a shared generic one, matching
/// this Unit's "minimal name and responsibility per work kind" scope. Never
/// persisted: an app restart has no running process to cancel anyway, and
/// `docs/PRODUCT_REQUIREMENTS.md`/AGENTS.md scope this Unit to an in-memory
/// handle, not a separate execution/session table.
#[derive(Clone, Default)]
pub struct ImplementationRunRegistry {
    inner: Arc<Mutex<HashMap<TaskId, AtomicCancellationSignal>>>,
}

impl ImplementationRunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh cancellation signal for `task_id`. Returns `None`
    /// instead of overwriting an existing entry: two live handles for the
    /// same task would mean two independent things could race to cancel (or
    /// fail to unregister) the same run. `TaskService::start_implementation`
    /// already fails closed (via optimistic task-version concurrency) before
    /// a second Implementation attempt for the same task could ever reach
    /// this call, so a `None` here indicates a caller invariant violation,
    /// not routine contention, and the caller must not silently proceed as
    /// if a handle had been registered.
    pub fn register(&self, task_id: TaskId) -> Option<AtomicCancellationSignal> {
        let signal = AtomicCancellationSignal::new();
        let mut guard = self.inner.lock().ok()?;
        if guard.contains_key(&task_id) {
            return None;
        }
        guard.insert(task_id, signal.clone());
        Some(signal)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }

    /// Requests cancellation of the in-flight run for `task_id`, if any.
    /// Returns whether a matching run was found; the caller must not infer
    /// anything about the eventual outcome from this alone — only a
    /// subsequently *confirmed* process exit is recorded as `Cancelled`.
    pub fn request_cancellation(&self, task_id: TaskId) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        let Some(signal) = guard.get(&task_id) else {
            return false;
        };
        signal.cancel();
        true
    }
}

/// In-memory-only registry preventing two concurrent `git merge --abort`
/// write attempts for the same task. Unlike
/// [`PlanningRunRegistry`]/[`ImplementationRunRegistry`]/[`TestingRunRegistry`]/[`ReviewRunRegistry`],
/// this registry never hands back a cancellation signal: a merge abort is a
/// single short-lived Git write with a fixed 20-second timeout that is never
/// interrupted mid-flight, and cancellation is explicitly out of scope for
/// this Unit (`docs/PHASE_PLAN.md` Phase 5e-4). Its sole purpose is
/// duplicate-execution prevention -- a second `confirm_merge_abort_and_start`
/// call for a task that already has one in flight must not spawn a second
/// background Git write against the same original checkout. Never
/// persisted: an app restart has no running process to duplicate against
/// anyway, and a task left `MergeConflict` after a restart is only ever
/// re-inspected read-only, never auto-retried (see
/// `TaskService::reconcile_startup_merge`, which this registry does not
/// participate in).
#[derive(Clone, Default)]
pub struct MergeAbortRunRegistry {
    inner: Arc<Mutex<HashSet<TaskId>>>,
}

impl MergeAbortRunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `task_id` as having an in-flight abort attempt. Returns
    /// `false` without registering anything if an entry already exists --
    /// the caller must not start a second background execution, must not
    /// treat this as an error, and must not have performed any Git write or
    /// approval write yet (registration is always the very first step).
    pub fn register(&self, task_id: TaskId) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        guard.insert(task_id)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }
}

/// In-memory-only mutual exclusion between the two `MergeConflict` recovery
/// paths that write to the *same original checkout*: merge-continue
/// (`git merge --continue`, `commands::merge_continue`) and merge-abort
/// (`git merge --abort`, `commands::merge_abort`). Both operate on one
/// task's single original checkout, and the two writes contradict each
/// other -- one completes the staged resolution as a merge commit, the
/// other discards it -- so starting them concurrently for the same task
/// would race two Git processes over the same index and `MERGE_HEAD`.
///
/// This lock is neither persistent state nor an approval identity: it
/// records only "a merge-conflict write for this task is executing right
/// now". It carries no cancellation signal (neither write is ever
/// interrupted mid-flight), no task version binding (the immutable
/// approvals and confirmations already carry that, and each command
/// re-verifies state/version transactionally), and nothing durable. Held
/// strictly for the duration of the write: every acquisition is released
/// by an RAII guard, including on background-thread panic and on uncertain
/// outcomes -- an entry is never left behind as a recovery marker.
///
/// Deliberately separate from [`MergeAbortRunRegistry`], which prevents a
/// *second abort* for the same task and is not generalized or renamed for
/// this cross-command role: a started abort holds both, and their release
/// conditions are documented independently.
///
/// An app restart clears this lock along with the rest of the process
/// memory. That is correct: a restart has no running Git process left to
/// exclude against, and a task left in `MergeConflict` (or `Merging`) is
/// recovered by the existing startup reconciliation
/// (`TaskService::reconcile_startup_merge`), which this lock does not
/// participate in.
#[derive(Clone, Default)]
pub struct MergeConflictWriteLock {
    inner: Arc<Mutex<HashSet<TaskId>>>,
}

impl MergeConflictWriteLock {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires the lock for `task_id`. Returns `false` without recording
    /// anything if a merge-conflict write for that task is already in
    /// flight -- the caller must not start a Git write, must not record an
    /// approval or confirmation, must not commit a state transition, and
    /// must not spawn a background thread.
    pub fn register(&self, task_id: TaskId) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        guard.insert(task_id)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }

    /// Read-only: whether a merge-conflict write for `task_id` is executing
    /// right now. Observes the same shared set every clone of this handle
    /// sees and mutates nothing -- it neither acquires nor releases the
    /// lock, so it can never be mistaken for a `register`.
    ///
    /// This exists so the UI can gate its merge-continue/merge-abort
    /// actions on the *authoritative* in-flight state rather than on a
    /// local flag that a polling tick would clear while the write is still
    /// running. It stays a boolean on purpose: which of the two writes is
    /// running, and everything it touches, is not the UI's business.
    ///
    /// A poisoned mutex reports `true`, matching `register`'s fail-closed
    /// direction: an unreadable lock must never be presented as "nothing is
    /// running, go ahead".
    #[must_use]
    pub fn is_running(&self, task_id: TaskId) -> bool {
        self.inner
            .lock()
            .map_or(true, |guard| guard.contains(&task_id))
    }
}

/// In-memory-only registry of cancellation handles for Cargo-only Testing
/// batches currently executing on a background thread, keyed by task id.
/// Mirrors [`PlanningRunRegistry`]/[`ImplementationRunRegistry`] exactly
/// (same shape, same invariants) but is kept as a separate type rather than a
/// shared generic one, matching this Unit's "minimal name and responsibility
/// per work kind" scope. Never persisted: an app restart has no running
/// process to cancel anyway, and `docs/PRODUCT_REQUIREMENTS.md`/AGENTS.md
/// scope this Unit to an in-memory handle, not a separate execution/session
/// table.
#[derive(Clone, Default)]
pub struct TestingRunRegistry {
    inner: Arc<Mutex<HashMap<TaskId, AtomicCancellationSignal>>>,
}

impl TestingRunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh cancellation signal for `task_id`. Returns `None`
    /// instead of overwriting an existing entry: two live handles for the
    /// same task would mean two independent things could race to cancel (or
    /// fail to unregister) the same run. Unlike Planning/Implementation,
    /// starting a Testing batch commits no state transition (the task is
    /// already `Testing`), so a `None` here means the caller must not spawn a
    /// second run and must leave the task's state untouched — there is
    /// nothing to fall back or recover from.
    pub fn register(&self, task_id: TaskId) -> Option<AtomicCancellationSignal> {
        let signal = AtomicCancellationSignal::new();
        let mut guard = self.inner.lock().ok()?;
        if guard.contains_key(&task_id) {
            return None;
        }
        guard.insert(task_id, signal.clone());
        Some(signal)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }

    /// Requests cancellation of the in-flight run for `task_id`, if any.
    /// Returns whether a matching run was found; the caller must not infer
    /// anything about the eventual outcome from this alone — only a
    /// subsequently *confirmed* process exit is recorded as `Paused`.
    pub fn request_cancellation(&self, task_id: TaskId) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        let Some(signal) = guard.get(&task_id) else {
            return false;
        };
        signal.cancel();
        true
    }
}

/// In-memory-only registry of cancellation handles for Claude Review runs
/// currently executing on a background thread, keyed by task id. Mirrors
/// [`PlanningRunRegistry`]/[`ImplementationRunRegistry`]/[`TestingRunRegistry`]
/// exactly (same shape, same invariants) but is kept as a separate type
/// rather than a shared generic one, matching this Unit's "minimal name and
/// responsibility per work kind" scope. Never persisted: an app restart has
/// no running process to cancel anyway, and
/// `docs/PRODUCT_REQUIREMENTS.md`/AGENTS.md scope this Unit to an in-memory
/// handle, not a separate execution/session table.
#[derive(Clone, Default)]
pub struct ReviewRunRegistry {
    inner: Arc<Mutex<HashMap<TaskId, AtomicCancellationSignal>>>,
}

impl ReviewRunRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh cancellation signal for `task_id`. Returns `None`
    /// instead of overwriting an existing entry: two live handles for the
    /// same task would mean two independent things could race to cancel (or
    /// fail to unregister) the same run. Unlike Planning/Implementation,
    /// starting a Claude Review run commits no state transition of its own
    /// (only a same-version consent, via `TaskService::start_review`), so a
    /// `None` here means the caller must not spawn a second run — there is
    /// no state transition to fall back or recover from, only a typed error
    /// leaving the task exactly as it was.
    pub fn register(&self, task_id: TaskId) -> Option<AtomicCancellationSignal> {
        let signal = AtomicCancellationSignal::new();
        let mut guard = self.inner.lock().ok()?;
        if guard.contains_key(&task_id) {
            return None;
        }
        guard.insert(task_id, signal.clone());
        Some(signal)
    }

    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&task_id);
        }
    }

    /// Requests cancellation of the in-flight run for `task_id`, if any.
    /// Returns whether a matching run was found; the caller must not infer
    /// anything about the eventual outcome from this alone — only a
    /// subsequently *confirmed* process exit is recorded as `Paused`.
    pub fn request_cancellation(&self, task_id: TaskId) -> bool {
        let Ok(guard) = self.inner.lock() else {
            return false;
        };
        let Some(signal) = guard.get(&task_id) else {
            return false;
        };
        signal.cancel();
        true
    }
}

#[derive(Clone, Default)]
pub struct RuntimeResources {
    pub paths: SharedResolvedAppPaths,
    pub database: SharedDatabase,
    pub logging_guard: SharedLoggingGuard,
}

#[derive(Clone)]
pub struct AppRuntime {
    pub bootstrap_status: BootstrapStatus,
    pub repository: RepositoryHandle,
    pub time: TimeProviderHandle,
    pub capabilities: CapabilityHandle,
    pub git: GitServiceHandle,
    pub filesystem: FilesystemIdentityHandle,
    pub worktree_paths: WorktreePathHandle,
    pub provider_capabilities: ProviderCapabilityHandle,
    pub preflight_dir: Option<PreflightDirectory>,
    pub planning_runs: PlanningRunRegistry,
    pub implementation_runs: ImplementationRunRegistry,
    pub testing_runs: TestingRunRegistry,
    pub review_runs: ReviewRunRegistry,
    pub merge_abort_runs: MergeAbortRunRegistry,
    pub merge_conflict_writes: MergeConflictWriteLock,
    resources: RuntimeResources,
}

pub struct RuntimePorts {
    pub repository: RepositoryHandle,
    pub time: TimeProviderHandle,
    pub capabilities: CapabilityHandle,
    pub git: GitServiceHandle,
    pub filesystem: FilesystemIdentityHandle,
    pub worktree_paths: WorktreePathHandle,
    pub provider_capabilities: ProviderCapabilityHandle,
    pub preflight_dir: Option<PreflightDirectory>,
    pub planning_runs: PlanningRunRegistry,
    pub implementation_runs: ImplementationRunRegistry,
    pub testing_runs: TestingRunRegistry,
    pub review_runs: ReviewRunRegistry,
    pub merge_abort_runs: MergeAbortRunRegistry,
    pub merge_conflict_writes: MergeConflictWriteLock,
}

impl AppRuntime {
    pub fn new(
        bootstrap_status: BootstrapStatus,
        ports: RuntimePorts,
        resources: RuntimeResources,
    ) -> Self {
        Self {
            bootstrap_status,
            repository: ports.repository,
            time: ports.time,
            capabilities: ports.capabilities,
            git: ports.git,
            filesystem: ports.filesystem,
            worktree_paths: ports.worktree_paths,
            provider_capabilities: ports.provider_capabilities,
            preflight_dir: ports.preflight_dir,
            planning_runs: ports.planning_runs,
            implementation_runs: ports.implementation_runs,
            testing_runs: ports.testing_runs,
            review_runs: ports.review_runs,
            merge_abort_runs: ports.merge_abort_runs,
            merge_conflict_writes: ports.merge_conflict_writes,
            resources,
        }
    }

    #[must_use]
    pub fn logging_guard_is_initialized(&self) -> bool {
        self.resources.logging_guard.is_initialized()
    }

    #[must_use]
    pub fn database_is_initialized(&self) -> bool {
        self.resources.database.is_initialized()
    }

    #[must_use]
    pub fn has_resolved_paths(&self) -> bool {
        self.resources
            .paths
            .lock()
            .map(|paths| paths.is_some())
            .unwrap_or(false)
    }

    /// The app-owned temp directory Cargo-only Testing execution uses for its
    /// fully `env_clear`'d child process `TEMP`/`TMP` (never the worktree,
    /// never an inherited value). `None` before app paths have resolved,
    /// which callers must treat the same as any other missing capability:
    /// fail closed, spawn nothing.
    #[must_use]
    pub fn app_temp_dir(&self) -> Option<std::path::PathBuf> {
        self.resources
            .paths
            .lock()
            .ok()
            .and_then(|paths| paths.as_ref().map(|resolved| resolved.temp_dir.clone()))
    }
}

pub struct UnavailableRuntime {
    pub error: ApplicationError,
    pub bootstrap_status: Option<BootstrapStatus>,
    pub migration_diagnostic: Option<LegacyMigrationDiagnostic>,
}

pub enum RuntimeState {
    Ready(AppRuntime),
    Unavailable(UnavailableRuntime),
}

pub enum RuntimeSnapshot {
    Ready(AppRuntime),
    Unavailable {
        error: ApplicationError,
        bootstrap_status: Option<BootstrapStatus>,
        migration_diagnostic: Option<LegacyMigrationDiagnostic>,
    },
}

pub struct ManagedRuntime {
    inner: Mutex<RuntimeState>,
}

impl ManagedRuntime {
    #[must_use]
    pub fn ready(runtime: AppRuntime) -> Self {
        Self {
            inner: Mutex::new(RuntimeState::Ready(runtime)),
        }
    }

    #[must_use]
    pub fn unavailable(error: ApplicationError, bootstrap_status: Option<BootstrapStatus>) -> Self {
        Self {
            inner: Mutex::new(RuntimeState::Unavailable(UnavailableRuntime {
                error,
                bootstrap_status,
                migration_diagnostic: None,
            })),
        }
    }

    #[must_use]
    pub fn unavailable_with_migration_diagnostic(
        error: ApplicationError,
        bootstrap_status: Option<BootstrapStatus>,
        migration_diagnostic: Option<LegacyMigrationDiagnostic>,
    ) -> Self {
        Self {
            inner: Mutex::new(RuntimeState::Unavailable(UnavailableRuntime {
                error,
                bootstrap_status,
                migration_diagnostic,
            })),
        }
    }

    pub fn lock(&self) -> Result<MutexGuard<'_, RuntimeState>, IpcErrorDto> {
        self.inner.lock().map_err(|_| IpcErrorDto::internal())
    }

    pub fn snapshot(&self) -> Result<RuntimeSnapshot, IpcErrorDto> {
        let state = self.lock()?;
        Ok(match &*state {
            RuntimeState::Ready(ready) => RuntimeSnapshot::Ready(ready.clone()),
            RuntimeState::Unavailable(unavailable) => RuntimeSnapshot::Unavailable {
                error: unavailable.error.clone(),
                bootstrap_status: unavailable.bootstrap_status.clone(),
                migration_diagnostic: unavailable.migration_diagnostic.clone(),
            },
        })
    }

    pub fn ready_snapshot(&self) -> Result<AppRuntime, IpcErrorDto> {
        match self.snapshot()? {
            RuntimeSnapshot::Ready(ready) => Ok(ready),
            RuntimeSnapshot::Unavailable { error, .. } => Err(error.into()),
        }
    }
}

impl From<SharedFoundationRepository> for RepositoryHandle {
    fn from(repository: SharedFoundationRepository) -> Self {
        Self::new(repository)
    }
}

#[cfg(test)]
mod planning_run_registry_tests {
    use super::PlanningRunRegistry;
    use chatoms_domain::TaskId;
    use chatoms_ports::process::CancellationSignal;

    #[test]
    fn cancellation_reaches_the_registered_signal_and_reports_it_was_found() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(!signal.is_cancelled());
        assert!(registry.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn cancellation_for_an_unregistered_task_reports_nothing_found() {
        let registry = PlanningRunRegistry::new();
        assert!(!registry.request_cancellation(TaskId::new()));
    }

    #[test]
    fn unregistering_removes_the_entry_so_a_later_cancel_reports_nothing_found() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let _signal = registry.register(task_id).expect("first registration");

        registry.unregister(task_id);

        assert!(!registry.request_cancellation(task_id));
    }

    #[test]
    fn a_clone_shares_the_same_underlying_registry() {
        let registry = PlanningRunRegistry::new();
        let clone = registry.clone();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(clone.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn registering_again_for_the_same_task_id_is_rejected_and_the_prior_signal_still_works() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");

        assert!(
            registry.register(task_id).is_none(),
            "a second registration for a task id already registered must be rejected"
        );

        assert!(registry.request_cancellation(task_id));
        assert!(
            first.is_cancelled(),
            "the original signal must still be the one in the registry"
        );
    }

    #[test]
    fn registering_again_after_unregistering_succeeds() {
        let registry = PlanningRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");
        registry.unregister(task_id);

        let second = registry
            .register(task_id)
            .expect("registration after unregister must succeed");

        assert!(registry.request_cancellation(task_id));
        assert!(
            !first.is_cancelled(),
            "the stale signal must not be reachable anymore"
        );
        assert!(second.is_cancelled());
    }
}

#[cfg(test)]
mod merge_abort_run_registry_tests {
    use super::MergeAbortRunRegistry;
    use chatoms_domain::TaskId;

    #[test]
    fn the_first_registration_for_a_task_id_succeeds() {
        let registry = MergeAbortRunRegistry::new();
        assert!(registry.register(TaskId::new()));
    }

    #[test]
    fn a_second_registration_for_the_same_task_id_is_rejected() {
        let registry = MergeAbortRunRegistry::new();
        let task_id = TaskId::new();
        assert!(registry.register(task_id));

        assert!(
            !registry.register(task_id),
            "a second concurrent registration for the same task id must be rejected"
        );
    }

    #[test]
    fn unregistering_allows_a_later_registration_to_succeed_again() {
        let registry = MergeAbortRunRegistry::new();
        let task_id = TaskId::new();
        assert!(registry.register(task_id));

        registry.unregister(task_id);

        assert!(
            registry.register(task_id),
            "registration after unregister must succeed"
        );
    }

    #[test]
    fn a_clone_shares_the_same_underlying_registry() {
        let registry = MergeAbortRunRegistry::new();
        let clone = registry.clone();
        let task_id = TaskId::new();
        assert!(registry.register(task_id));

        assert!(
            !clone.register(task_id),
            "a clone must observe the same registered task id"
        );
    }

    #[test]
    fn unregistering_an_unregistered_task_id_is_a_no_op() {
        let registry = MergeAbortRunRegistry::new();
        registry.unregister(TaskId::new());
        // No panic, and a fresh registration for a different id still works.
        assert!(registry.register(TaskId::new()));
    }
}

#[cfg(test)]
mod merge_conflict_write_lock_tests {
    use super::MergeConflictWriteLock;
    use chatoms_domain::TaskId;

    #[test]
    fn the_first_acquisition_for_a_task_id_succeeds() {
        let lock = MergeConflictWriteLock::new();
        assert!(lock.register(TaskId::new()));
    }

    #[test]
    fn a_second_acquisition_for_the_same_task_id_is_rejected() {
        let lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(lock.register(task_id));

        assert!(
            !lock.register(task_id),
            "a merge-conflict write already in flight for this task must exclude a second one"
        );
    }

    #[test]
    fn an_acquisition_for_a_different_task_id_is_not_blocked() {
        let lock = MergeConflictWriteLock::new();
        assert!(lock.register(TaskId::new()));

        assert!(
            lock.register(TaskId::new()),
            "the lock is per task: a different task's original checkout is not excluded"
        );
    }

    #[test]
    fn releasing_allows_a_later_acquisition_to_succeed_again() {
        let lock = MergeConflictWriteLock::new();
        let task_id = TaskId::new();
        assert!(lock.register(task_id));

        lock.unregister(task_id);

        assert!(
            lock.register(task_id),
            "acquisition after release must succeed: the lock is not a recovery marker"
        );
    }

    #[test]
    fn a_clone_shares_the_same_underlying_lock() {
        let lock = MergeConflictWriteLock::new();
        let clone = lock.clone();
        let task_id = TaskId::new();
        assert!(lock.register(task_id));

        assert!(
            !clone.register(task_id),
            "a clone handed to a background thread must observe the same held lock"
        );

        clone.unregister(task_id);

        assert!(
            lock.register(task_id),
            "a release through one clone must be observable through the other"
        );
    }

    #[test]
    fn releasing_an_unheld_task_id_is_a_no_op() {
        let lock = MergeConflictWriteLock::new();
        lock.unregister(TaskId::new());
        // No panic, and a fresh acquisition for a different id still works.
        assert!(lock.register(TaskId::new()));
    }
}

#[cfg(test)]
mod implementation_run_registry_tests {
    use super::ImplementationRunRegistry;
    use chatoms_domain::TaskId;
    use chatoms_ports::process::CancellationSignal;

    #[test]
    fn cancellation_reaches_the_registered_signal_and_reports_it_was_found() {
        let registry = ImplementationRunRegistry::new();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(!signal.is_cancelled());
        assert!(registry.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn cancellation_for_an_unregistered_task_reports_nothing_found() {
        let registry = ImplementationRunRegistry::new();
        assert!(!registry.request_cancellation(TaskId::new()));
    }

    #[test]
    fn unregistering_removes_the_entry_so_a_later_cancel_reports_nothing_found() {
        let registry = ImplementationRunRegistry::new();
        let task_id = TaskId::new();
        let _signal = registry.register(task_id).expect("first registration");

        registry.unregister(task_id);

        assert!(!registry.request_cancellation(task_id));
    }

    #[test]
    fn a_clone_shares_the_same_underlying_registry() {
        let registry = ImplementationRunRegistry::new();
        let clone = registry.clone();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(clone.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn registering_again_for_the_same_task_id_is_rejected_and_the_prior_signal_still_works() {
        let registry = ImplementationRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");

        assert!(
            registry.register(task_id).is_none(),
            "a second registration for a task id already registered must be rejected"
        );

        assert!(registry.request_cancellation(task_id));
        assert!(
            first.is_cancelled(),
            "the original signal must still be the one in the registry"
        );
    }

    #[test]
    fn registering_again_after_unregistering_succeeds() {
        let registry = ImplementationRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");
        registry.unregister(task_id);

        let second = registry
            .register(task_id)
            .expect("registration after unregister must succeed");

        assert!(registry.request_cancellation(task_id));
        assert!(
            !first.is_cancelled(),
            "the stale signal must not be reachable anymore"
        );
        assert!(second.is_cancelled());
    }
}

#[cfg(test)]
mod testing_run_registry_tests {
    use super::TestingRunRegistry;
    use chatoms_domain::TaskId;
    use chatoms_ports::process::CancellationSignal;

    #[test]
    fn cancellation_reaches_the_registered_signal_and_reports_it_was_found() {
        let registry = TestingRunRegistry::new();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(!signal.is_cancelled());
        assert!(registry.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn cancellation_for_an_unregistered_task_reports_nothing_found() {
        let registry = TestingRunRegistry::new();
        assert!(!registry.request_cancellation(TaskId::new()));
    }

    #[test]
    fn unregistering_removes_the_entry_so_a_later_cancel_reports_nothing_found() {
        let registry = TestingRunRegistry::new();
        let task_id = TaskId::new();
        let _signal = registry.register(task_id).expect("first registration");

        registry.unregister(task_id);

        assert!(!registry.request_cancellation(task_id));
    }

    #[test]
    fn a_clone_shares_the_same_underlying_registry() {
        let registry = TestingRunRegistry::new();
        let clone = registry.clone();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(clone.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn registering_again_for_the_same_task_id_is_rejected_and_the_prior_signal_still_works() {
        let registry = TestingRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");

        assert!(
            registry.register(task_id).is_none(),
            "a second registration for a task id already registered must be rejected"
        );

        assert!(registry.request_cancellation(task_id));
        assert!(
            first.is_cancelled(),
            "the original signal must still be the one in the registry"
        );
    }

    #[test]
    fn registering_again_after_unregistering_succeeds() {
        let registry = TestingRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");
        registry.unregister(task_id);

        let second = registry
            .register(task_id)
            .expect("registration after unregister must succeed");

        assert!(registry.request_cancellation(task_id));
        assert!(
            !first.is_cancelled(),
            "the stale signal must not be reachable anymore"
        );
        assert!(second.is_cancelled());
    }
}

#[cfg(test)]
mod review_run_registry_tests {
    use super::ReviewRunRegistry;
    use chatoms_domain::TaskId;
    use chatoms_ports::process::CancellationSignal;

    #[test]
    fn cancellation_reaches_the_registered_signal_and_reports_it_was_found() {
        let registry = ReviewRunRegistry::new();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(!signal.is_cancelled());
        assert!(registry.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn cancellation_for_an_unregistered_task_reports_nothing_found() {
        let registry = ReviewRunRegistry::new();
        assert!(!registry.request_cancellation(TaskId::new()));
    }

    #[test]
    fn unregistering_removes_the_entry_so_a_later_cancel_reports_nothing_found() {
        let registry = ReviewRunRegistry::new();
        let task_id = TaskId::new();
        let _signal = registry.register(task_id).expect("first registration");

        registry.unregister(task_id);

        assert!(!registry.request_cancellation(task_id));
    }

    #[test]
    fn a_clone_shares_the_same_underlying_registry() {
        let registry = ReviewRunRegistry::new();
        let clone = registry.clone();
        let task_id = TaskId::new();
        let signal = registry.register(task_id).expect("first registration");

        assert!(clone.request_cancellation(task_id));
        assert!(signal.is_cancelled());
    }

    #[test]
    fn registering_again_for_the_same_task_id_is_rejected_and_the_prior_signal_still_works() {
        let registry = ReviewRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");

        assert!(
            registry.register(task_id).is_none(),
            "a second registration for a task id already registered must be rejected"
        );

        assert!(registry.request_cancellation(task_id));
        assert!(
            first.is_cancelled(),
            "the original signal must still be the one in the registry"
        );
    }

    #[test]
    fn registering_again_after_unregistering_succeeds() {
        let registry = ReviewRunRegistry::new();
        let task_id = TaskId::new();
        let first = registry.register(task_id).expect("first registration");
        registry.unregister(task_id);

        let second = registry
            .register(task_id)
            .expect("registration after unregister must succeed");

        assert!(registry.request_cancellation(task_id));
        assert!(
            !first.is_cancelled(),
            "the stale signal must not be reachable anymore"
        );
        assert!(second.is_cancelled());
    }
}

#[cfg(test)]
mod filesystem_identity_handle_tests {
    use super::FilesystemIdentityHandle;
    use chatoms_ports::{
        error::{FailureCategory, PortFailure},
        filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    };

    struct RecordingFilesystemIdentity;

    impl FilesystemIdentityPort for RecordingFilesystemIdentity {
        fn inspect_supported_directory(
            &mut self,
            _path: &std::path::Path,
        ) -> Result<DirectoryIdentity, PortFailure> {
            Err(PortFailure::new(FailureCategory::Unsupported))
        }

        fn verify_local_tree(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
            Err(PortFailure::new(FailureCategory::Unsupported))
        }

        fn acquire_guard(
            &mut self,
            _path: &std::path::Path,
            _expected: &DirectoryIdentity,
        ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
            Err(PortFailure::new(FailureCategory::Unsupported))
        }

        fn inspect_supported_file(
            &mut self,
            _path: &std::path::Path,
        ) -> Result<DirectoryIdentity, PortFailure> {
            Ok(DirectoryIdentity {
                canonical_path: std::path::PathBuf::from("C:/tools/cargo.exe"),
                volume_serial_hex: "0000000000000001".to_owned(),
                file_id_hex: "00000000000000000000000000000001".to_owned(),
            })
        }
    }

    /// Regression test: `inspect_supported_file` has a trait-default
    /// fail-closed `Unsupported` fallback (see
    /// `chatoms_ports::filesystem::FilesystemIdentityPort`'s own docs), which
    /// a wrapper that forgets to delegate this one method silently inherits
    /// instead of ever reaching the wrapped adapter — the same shape of bug
    /// `RepositoryHandle`'s wrapper chain hit before (see
    /// `src-tauri/tests/repository_handle_wiring.rs`). Every validation
    /// command approval/verification call goes through this handle, so a
    /// missing delegation here would silently and permanently reject every
    /// approval as `Unsupported`.
    #[test]
    fn inspect_supported_file_delegates_to_the_wrapped_adapter_instead_of_the_trait_default() {
        let mut handle = FilesystemIdentityHandle::new(RecordingFilesystemIdentity);

        let identity = handle
            .inspect_supported_file(std::path::Path::new("C:/tools/cargo.exe"))
            .expect("the wrapped adapter's Ok result must be returned, not the trait default");

        assert_eq!(identity.volume_serial_hex, "0000000000000001");
    }
}
