use std::path::Path;
use std::sync::{Arc, Mutex};

use chatoms_domain::{
    ContextDataScope, HighRiskCategory, OperationRiskKind, ProjectId, Task, TaskId,
    TaskStateTransition, ValidationCommandKind, ValidationExecutionScope, WorkKind,
};
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState, LoggingBootstrapPort, LoggingBootstrapState,
    diff::DiffContentHash,
    error::{CategorizedFailure, FailureCategory, PortFailure},
    filesystem::FilesystemIdentityPort,
    git::{GitService, RepositoryKind},
    manual_merge_resolution::ManualResolutionDigest,
    path::ResolvedAppPaths,
    permissions::PermissionStatus,
    provider::ProviderKind,
    repository::{
        ActiveLease, AppProfileRecord, ContextPackageManifestRecord, ContextPackagePreparation,
        DiffApprovalRecord, FoundationRepository, GitInitApproval, GitOperationAttempt,
        GitOperationReceipt, GitOperationReceiptKind, HighRiskApprovalRecord,
        ManualMergeResolutionConfirmationRecord, MergeAbortApprovalRecord,
        OperationRiskDeclaration, OperationRiskDeclarationRecord, PostMergeValidationResultAttempt,
        PostMergeValidationResultRecord, ProjectFilesystemIdentityRecord, ProjectRecord,
        ProjectSummary, ProviderBindingRecord, ProviderConsent, RepositoryError,
        RepositoryErrorCode, TaskBriefRecord, TaskGitIsolation, TaskImplementationResultRecord,
        TaskPlanningResultRecord, TaskReviewResultRecord, ValidationCommandApprovalRecord,
        ValidationCommandResultAttempt, ValidationCommandResultRecord,
    },
};

use crate::{
    database::{
        DatabaseConnection, DatabaseError, LegacyProject, LegacyProjectIdentity,
        LegacyProjectPreflight, MigrationRunner, SqliteFoundationRepository,
    },
    logging::{LogLevel, LoggingConfig, LoggingGuard, ValidatedLogDirectory, initialize_logging},
};

pub type SharedResolvedAppPaths = Arc<Mutex<Option<ResolvedAppPaths>>>;

#[derive(Clone, Default)]
pub struct SharedDatabase {
    inner: Arc<Mutex<Option<DatabaseConnection>>>,
    migration_diagnostic: Arc<Mutex<Option<LegacyMigrationDiagnostic>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyMigrationDiagnostic {
    pub project_id: String,
    pub display_path: String,
    pub reason_code: &'static str,
}

impl SharedDatabase {
    #[must_use]
    pub fn repository(&self) -> SharedFoundationRepository {
        SharedFoundationRepository {
            database: self.clone(),
        }
    }

    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.inner
            .lock()
            .map(|database| database.is_some())
            .unwrap_or(false)
    }

    #[must_use]
    pub fn migration_diagnostic(&self) -> Option<LegacyMigrationDiagnostic> {
        self.migration_diagnostic
            .lock()
            .ok()
            .and_then(|diagnostic| diagnostic.clone())
    }
}

pub struct DatabaseBootstrapAdapter {
    paths: SharedResolvedAppPaths,
    database: SharedDatabase,
    legacy_preflight: Option<Box<dyn LegacyProjectPreflight + Send>>,
}

impl DatabaseBootstrapAdapter {
    #[must_use]
    pub const fn new(paths: SharedResolvedAppPaths, database: SharedDatabase) -> Self {
        Self {
            paths,
            database,
            legacy_preflight: None,
        }
    }

    #[must_use]
    pub fn with_legacy_preflight(
        mut self,
        preflight: impl LegacyProjectPreflight + Send + 'static,
    ) -> Self {
        self.legacy_preflight = Some(Box::new(preflight));
        self
    }
}

impl DatabaseBootstrapPort for DatabaseBootstrapAdapter {
    fn bootstrap_database(&mut self) -> Result<DatabaseBootstrapState, PortFailure> {
        if self.database.is_initialized() {
            return Ok(DatabaseBootstrapState::Ready);
        }
        let database_path = self
            .paths
            .lock()
            .map_err(|_| internal_failure())?
            .as_ref()
            .map(|paths| paths.database_path.clone())
            .ok_or_else(storage_unavailable)?;
        let mut connection = DatabaseConnection::open(database_path).map_err(database_failure)?;
        let runner = MigrationRunner::default();
        let migration_result = if let Some(preflight) = self.legacy_preflight.as_deref_mut() {
            runner.run_with_preflight(&mut connection, preflight)
        } else {
            runner.run(&mut connection)
        };
        let outcome = match migration_result {
            Ok(outcome) => outcome,
            Err(DatabaseError::DatabaseNewerThanApplication { .. }) => {
                return Ok(DatabaseBootstrapState::Incompatible);
            }
            Err(error) => {
                if let DatabaseError::LegacyProjectPreflightFailed {
                    project_id,
                    display_path,
                    reason,
                } = &error
                {
                    let mut diagnostic = self
                        .database
                        .migration_diagnostic
                        .lock()
                        .map_err(|_| internal_failure())?;
                    *diagnostic = Some(LegacyMigrationDiagnostic {
                        project_id: project_id.clone(),
                        display_path: display_path.clone(),
                        reason_code: reason,
                    });
                }
                return Err(database_failure(error));
            }
        };
        let status = if outcome.applied_count == 0 {
            DatabaseBootstrapState::Ready
        } else {
            DatabaseBootstrapState::Upgraded
        };
        let mut stored = self.database.inner.lock().map_err(|_| internal_failure())?;
        *stored = Some(connection);
        Ok(status)
    }
}

pub struct LegacyProjectPreflightAdapter<G, F> {
    git: G,
    filesystem: F,
}

impl<G, F> LegacyProjectPreflightAdapter<G, F> {
    #[must_use]
    pub const fn new(git: G, filesystem: F) -> Self {
        Self { git, filesystem }
    }
}

impl<G, F> LegacyProjectPreflight for LegacyProjectPreflightAdapter<G, F>
where
    G: GitService,
    F: FilesystemIdentityPort,
{
    fn resolve(
        &mut self,
        projects: &[LegacyProject],
    ) -> Result<Vec<LegacyProjectIdentity>, DatabaseError> {
        let mut identities = Vec::with_capacity(projects.len());
        for project in projects {
            // Do not let a legacy database path reach Git before it has passed the
            // same storage trust gate used for newly registered projects.
            let stored_root = self
                .filesystem
                .inspect_supported_directory(Path::new(&project.root_path))
                .map_err(|_| {
                    legacy_preflight_error(project, "stable root identity could not be confirmed")
                })?;
            self.filesystem
                .verify_local_tree(&stored_root.canonical_path)
                .map_err(|_| {
                    legacy_preflight_error(project, "project contains cloud or unsupported content")
                })?;
            let inspection = self
                .git
                .inspect_project(&stored_root.canonical_path)
                .map_err(|_| {
                    legacy_preflight_error(
                        project,
                        "project root is missing, remote, or unsupported",
                    )
                })?;
            let root = self
                .filesystem
                .inspect_supported_directory(&inspection.canonical_root)
                .map_err(|_| {
                    legacy_preflight_error(project, "stable root identity could not be confirmed")
                })?;
            self.filesystem
                .verify_local_tree(&root.canonical_path)
                .map_err(|_| {
                    legacy_preflight_error(project, "project contains cloud or unsupported content")
                })?;
            let common = inspection
                .git_common_dir
                .as_deref()
                .map(|path| self.filesystem.inspect_supported_directory(path))
                .transpose()
                .map_err(|_| {
                    legacy_preflight_error(
                        project,
                        "Git common directory identity could not be confirmed",
                    )
                })?;
            if inspection.repository_kind == RepositoryKind::Git && common.is_none() {
                return Err(legacy_preflight_error(
                    project,
                    "Git common directory identity is missing",
                ));
            }
            identities.push(LegacyProjectIdentity {
                project_id: project.project_id.clone(),
                canonical_path_key: inspection.canonical_key,
                display_path: inspection.display_path,
                root_volume_serial_hex: root.volume_serial_hex,
                root_file_id_hex: root.file_id_hex,
                repository_kind: match inspection.repository_kind {
                    RepositoryKind::Git => "Git",
                    RepositoryKind::NonGit => "NonGit",
                },
                git_common_volume_serial_hex: common
                    .as_ref()
                    .map(|identity| identity.volume_serial_hex.clone()),
                git_common_file_id_hex: common.map(|identity| identity.file_id_hex),
            });
        }
        Ok(identities)
    }
}

fn legacy_preflight_error(project: &LegacyProject, reason: &'static str) -> DatabaseError {
    DatabaseError::LegacyProjectPreflightFailed {
        project_id: project.project_id.clone(),
        display_path: legacy_display_hint(project),
        reason,
    }
}

fn legacy_display_hint(project: &LegacyProject) -> String {
    let tail = Path::new(&project.root_path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter(|component| !component.is_empty())
        .rev()
        .take(2)
        .collect::<Vec<_>>();
    if tail.is_empty() {
        project.name.clone()
    } else {
        format!(
            "…\\{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\\")
        )
    }
}

#[derive(Clone, Default)]
pub struct SharedLoggingGuard {
    inner: Arc<Mutex<Option<LoggingGuard>>>,
}

impl SharedLoggingGuard {
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

pub struct LoggingBootstrapAdapter {
    paths: SharedResolvedAppPaths,
    guard: SharedLoggingGuard,
}

impl LoggingBootstrapAdapter {
    #[must_use]
    pub const fn new(paths: SharedResolvedAppPaths, guard: SharedLoggingGuard) -> Self {
        Self { paths, guard }
    }
}

impl LoggingBootstrapPort for LoggingBootstrapAdapter {
    fn bootstrap_logging(&mut self) -> Result<LoggingBootstrapState, PortFailure> {
        if self.guard.is_initialized() {
            return Ok(LoggingBootstrapState::Ready);
        }
        let paths = self
            .paths
            .lock()
            .map_err(|_| internal_failure())?
            .clone()
            .ok_or_else(storage_unavailable)?;
        let directory = ValidatedLogDirectory::from_secure_paths(&paths, PermissionStatus::Secure)
            .map_err(categorized_failure)?;
        let guard = initialize_logging(&LoggingConfig::new(directory, LogLevel::Info))
            .map_err(categorized_failure)?;
        let mut stored = self.guard.inner.lock().map_err(|_| internal_failure())?;
        *stored = Some(guard);
        Ok(LoggingBootstrapState::Ready)
    }
}

#[derive(Clone)]
pub struct SharedFoundationRepository {
    database: SharedDatabase,
}

impl SharedFoundationRepository {
    fn with_repository<T>(
        &mut self,
        operation: impl FnOnce(&mut SqliteFoundationRepository<'_>) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut stored = self
            .database
            .inner
            .lock()
            .map_err(|_| RepositoryError::new(RepositoryErrorCode::DatabaseUnavailable))?;
        let database = stored
            .as_mut()
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::DatabaseUnavailable))?;
        operation(&mut SqliteFoundationRepository::new(database))
    }
}

impl FoundationRepository for SharedFoundationRepository {
    fn create_project(&mut self, project: &ProjectRecord) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.create_project(project))
    }

    fn create_project_with_identity(
        &mut self,
        project: &ProjectRecord,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.create_project_with_identity(project, identity)
        })
    }

    fn get_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_project_identity(project_id))
    }

    fn update_project_identity(
        &mut self,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.update_project_identity(identity))
    }

    fn get_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_project(project_id))
    }

    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.create_task(task, initial_transition, lease_acquired_at_ms)
        })
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.with_repository(|repository| repository.get_task(task_id))
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_transition(expected_version, task, transition)
        })
    }

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.save_recovery_target(expected_version, task))
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.terminate_task(expected_version, task, transition)
        })
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        self.with_repository(|repository| repository.list_task_transitions(task_id))
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.with_repository(|repository| repository.list_projects())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.with_repository(|repository| repository.active_lease())
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
        self.with_repository(|repository| {
            repository.create_isolation_task(
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
        self.with_repository(|repository| repository.get_task_isolation(task_id))
    }

    fn get_task_brief(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskBriefRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_task_brief(task_id))
    }

    fn get_task_planning_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskPlanningResultRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_task_planning_result(task_id))
    }

    fn get_task_implementation_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskImplementationResultRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_task_implementation_result(task_id))
    }

    fn get_task_review_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskReviewResultRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_task_review_result(task_id))
    }

    fn get_provider_consent(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_provider_consent(
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
        self.with_repository(|repository| {
            repository.save_planning_transition(expected_version, task, transition, consent)
        })
    }

    fn save_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_implementation_transition(expected_version, task, transition, consent)
        })
    }

    fn save_review_consent(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        data_scope: ContextDataScope,
        consented_at_ms: i64,
    ) -> Result<ProviderConsent, RepositoryError> {
        self.with_repository(|repository| {
            repository.save_review_consent(expected_version, task_id, data_scope, consented_at_ms)
        })
    }

    fn save_context_package_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_context_package_planning_transition(expected_version, task, transition)
        })
    }

    fn save_context_package_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_context_package_implementation_transition(
                expected_version,
                task,
                transition,
            )
        })
    }

    fn prepare_planning_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.with_repository(|repository| {
            repository.prepare_planning_context_package(expected_version, task_id, prepared_at_ms)
        })
    }

    fn prepare_implementation_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.with_repository(|repository| {
            repository.prepare_implementation_context_package(
                expected_version,
                task_id,
                prepared_at_ms,
            )
        })
    }

    fn prepare_review_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.with_repository(|repository| {
            repository.prepare_review_context_package(expected_version, task_id, prepared_at_ms)
        })
    }

    fn save_context_package_manifest(
        &mut self,
        record: &ContextPackageManifestRecord,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.save_context_package_manifest(record))
    }

    fn get_context_package_manifest(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ContextPackageManifestRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_context_package_manifest(
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
        self.with_repository(|repository| repository.save_high_risk_approval(approval))
    }

    fn get_high_risk_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_high_risk_approval(task_id, approved_task_version, risk_category)
        })
    }

    fn ensure_high_risk_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
        approved_at_ms: i64,
    ) -> Result<HighRiskApprovalRecord, RepositoryError> {
        self.with_repository(|repository| {
            repository.ensure_high_risk_approval(
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
        self.with_repository(|repository| {
            repository.declare_operation_risk(declaration, risk_categories)
        })
    }

    fn get_operation_risk_declaration(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        operation_kind: OperationRiskKind,
    ) -> Result<Option<OperationRiskDeclaration>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_operation_risk_declaration(
                task_id,
                approved_task_version,
                operation_kind,
            )
        })
    }

    fn save_diff_approval(&mut self, approval: &DiffApprovalRecord) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.save_diff_approval(approval))
    }

    fn get_diff_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        diff_content_hash: DiffContentHash,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_diff_approval(task_id, approved_task_version, diff_content_hash)
        })
    }

    fn get_diff_approval_for_task_version(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_diff_approval_for_task_version(task_id, approved_task_version)
        })
    }

    fn ensure_diff_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        diff_content_hash: DiffContentHash,
        approved_at_ms: i64,
    ) -> Result<DiffApprovalRecord, RepositoryError> {
        self.with_repository(|repository| {
            repository.ensure_diff_approval(
                task_id,
                expected_version,
                diff_content_hash,
                approved_at_ms,
            )
        })
    }

    fn get_manual_merge_resolution_confirmation(
        &mut self,
        task_id: TaskId,
        merge_conflict_task_version: u64,
        resolution_digest: ManualResolutionDigest,
    ) -> Result<Option<ManualMergeResolutionConfirmationRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.get_manual_merge_resolution_confirmation(
                task_id,
                merge_conflict_task_version,
                resolution_digest,
            )
        })
    }

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
        self.with_repository(|repository| {
            repository.ensure_manual_merge_resolution_confirmation(
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
        self.with_repository(|repository| {
            repository.save_manual_merge_resolution_transition(
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
        self.with_repository(|repository| {
            repository.get_merge_abort_approval(task_id, merge_conflict_task_version)
        })
    }

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
        self.with_repository(|repository| {
            repository.ensure_merge_abort_approval(
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
        self.with_repository(|repository| {
            repository.save_merge_abort_transition(expected_version, task, transition, terminal)
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
        self.with_repository(|repository| {
            repository.save_planning_result(expected_version, task, transition, result, terminal)
        })
    }

    fn save_implementation_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskImplementationResultRecord,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_implementation_result(expected_version, task, transition, result)
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
        self.with_repository(|repository| {
            repository.save_review_result(expected_version, task, transition, result, terminal)
        })
    }

    fn save_validation_command_approval(
        &mut self,
        approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| repository.save_validation_command_approval(approval))
    }

    fn list_validation_command_approvals(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.list_validation_command_approvals(task_id, approved_task_version)
        })
    }

    fn list_validation_command_approvals_for_scope(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        execution_scope: ValidationExecutionScope,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.list_validation_command_approvals_for_scope(
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
        self.with_repository(|repository| repository.append_validation_command_result(attempt))
    }

    fn list_validation_command_results(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<ValidationCommandResultRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.list_validation_command_results(task_id, approved_task_version, kind)
        })
    }

    fn finalize_validation_command_batch(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.finalize_validation_command_batch(
                expected_version,
                task,
                transition,
                attempt,
            )
        })
    }

    fn append_post_merge_validation_result(
        &mut self,
        attempt: &PostMergeValidationResultAttempt,
    ) -> Result<PostMergeValidationResultRecord, RepositoryError> {
        self.with_repository(|repository| repository.append_post_merge_validation_result(attempt))
    }

    fn list_post_merge_validation_results(
        &mut self,
        task_id: TaskId,
        approval_task_version: u64,
        post_merge_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<PostMergeValidationResultRecord>, RepositoryError> {
        self.with_repository(|repository| {
            repository.list_post_merge_validation_results(
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
        self.with_repository(|repository| {
            repository.finalize_post_merge_validation_batch(
                expected_version,
                task,
                transition,
                attempt,
            )
        })
    }

    fn begin_git_initialization(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
        approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.begin_git_initialization(expected_version, isolation, approval)
        })
    }

    fn save_isolation_intent(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_isolation_intent(expected_version, isolation)
        })
    }

    fn append_git_operation_receipt(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.append_git_operation_receipt(operation_id, kind, evidence, recorded_at_ms)
        })
    }

    fn list_git_operation_receipts(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        self.with_repository(|repository| repository.list_git_operation_receipts(operation_id))
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        self.with_repository(|repository| repository.list_incomplete_git_operations())
    }

    fn save_isolation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.save_isolation_transition(expected_version, task, transition, isolation)
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
        self.with_repository(|repository| {
            repository.save_git_initialization_completion(
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
        self.with_repository(|repository| {
            repository.save_worktree_completion(expected_version, task, transition, isolation)
        })
    }

    fn terminate_isolation_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.terminate_isolation_task(expected_version, task, transition, isolation)
        })
    }

    fn ensure_default_profile_and_claude_binding(
        &mut self,
        profile: &AppProfileRecord,
        binding: &ProviderBindingRecord,
    ) -> Result<ProviderBindingRecord, RepositoryError> {
        self.with_repository(|repository| {
            repository.ensure_default_profile_and_claude_binding(profile, binding)
        })
    }

    fn get_claude_binding(
        &mut self,
        profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        self.with_repository(|repository| repository.get_claude_binding(profile_name))
    }

    fn update_claude_executable_path(
        &mut self,
        binding_id: &str,
        executable_path: Option<&str>,
        updated_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.with_repository(|repository| {
            repository.update_claude_executable_path(binding_id, executable_path, updated_at_ms)
        })
    }
}

fn categorized_failure(error: impl CategorizedFailure) -> PortFailure {
    PortFailure::with_policy(error.category(), error.severity(), error.retry())
}

fn database_failure(error: DatabaseError) -> PortFailure {
    categorized_failure(error)
}

fn storage_unavailable() -> PortFailure {
    PortFailure::new(FailureCategory::StorageUnavailable)
}

fn internal_failure() -> PortFailure {
    PortFailure::new(FailureCategory::Internal)
}
