use chatoms_application::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
};
use chatoms_domain::{HighRiskCategory, Task, TaskId, TaskStateTransition};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    repository::{
        ActiveLease, FoundationRepository, HighRiskApprovalRecord, OperationRiskDeclaration,
        OperationRiskDeclarationRecord, ProjectSummary, RepositoryError, RepositoryErrorCode,
        TaskGitIsolation,
    },
};

use crate::state::{
    AppRuntime, CapabilityHandle, ManagedRuntime, RepositoryHandle, RuntimePorts, RuntimeResources,
    TimeProviderHandle,
};

use super::{
    CapabilityFake, GitCapabilityFake, TimeFake, operation_failed, worktree_ready_isolation,
};

struct OperationRiskRepositoryFake {
    task: Task,
    lease: ActiveLease,
    project: chatoms_ports::repository::ProjectRecord,
    project_identity: chatoms_ports::repository::ProjectFilesystemIdentityRecord,
    isolation: TaskGitIsolation,
    approvals: Vec<HighRiskApprovalRecord>,
    declaration: Option<OperationRiskDeclaration>,
    persistence_fails: bool,
}

impl FoundationRepository for OperationRiskRepositoryFake {
    fn create_task(
        &mut self,
        _task: &Task,
        _initial_transition: &TaskStateTransition,
        _lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        Ok((self.task.id() == task_id).then(|| self.task.clone()))
    }

    fn save_transition(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }

    fn save_recovery_target(
        &mut self,
        _expected_version: u64,
        _task: &Task,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }

    fn terminate_task(
        &mut self,
        _expected_version: u64,
        _task: &Task,
        _transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }

    fn list_task_transitions(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        Ok(Vec::new())
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        Ok(Vec::new())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        Ok(Some(self.lease))
    }

    fn get_project(
        &mut self,
        project_id: chatoms_domain::ProjectId,
    ) -> Result<Option<chatoms_ports::repository::ProjectRecord>, RepositoryError> {
        Ok((self.project.id == project_id).then(|| self.project.clone()))
    }

    fn get_project_identity(
        &mut self,
        project_id: chatoms_domain::ProjectId,
    ) -> Result<Option<chatoms_ports::repository::ProjectFilesystemIdentityRecord>, RepositoryError>
    {
        Ok((self.project_identity.project_id == project_id).then(|| self.project_identity.clone()))
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        Ok((self.isolation.task_id == task_id).then(|| self.isolation.clone()))
    }

    fn get_high_risk_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        Ok(self
            .approvals
            .iter()
            .find(|approval| {
                approval.task_id == task_id
                    && approval.approved_task_version == approved_task_version
                    && approval.risk_category == risk_category
            })
            .copied())
    }

    fn declare_operation_risk(
        &mut self,
        declaration: &OperationRiskDeclarationRecord,
        risk_categories: &[HighRiskCategory],
    ) -> Result<(), RepositoryError> {
        if self.persistence_fails {
            return Err(RepositoryError::new(
                RepositoryErrorCode::DatabaseUnavailable,
            ));
        }
        if self.declaration.is_some() {
            return Err(RepositoryError::new(
                RepositoryErrorCode::InvalidPersistenceState,
            ));
        }
        self.declaration = Some(OperationRiskDeclaration {
            record: *declaration,
            risk_categories: risk_categories.to_vec(),
        });
        Ok(())
    }

    fn get_operation_risk_declaration(
        &mut self,
        _task_id: TaskId,
        _approved_task_version: u64,
        _operation_kind: chatoms_domain::OperationRiskKind,
    ) -> Result<Option<OperationRiskDeclaration>, RepositoryError> {
        if self.persistence_fails {
            return Err(RepositoryError::new(
                RepositoryErrorCode::DatabaseUnavailable,
            ));
        }
        Ok(self.declaration.clone())
    }
}

pub(super) fn ready_runtime_for_operation_risk(
    task: Task,
    approvals: Vec<HighRiskApprovalRecord>,
    persistence_fails: bool,
    identity_matches: bool,
) -> ManagedRuntime {
    let root_path = "C:/chatoms-test/project".to_owned();
    let isolation = worktree_ready_isolation(
        task.id(),
        task.project_id(),
        std::path::Path::new("C:/chatoms-test/worktree"),
        task.version(),
    );
    let filesystem = if identity_matches {
        crate::state::FilesystemIdentityHandle::new(super::EchoFilesystemIdentity)
    } else {
        crate::state::FilesystemIdentityHandle::new(MismatchedFilesystemIdentity)
    };
    ManagedRuntime::ready(AppRuntime::new(
        BootstrapStatus {
            storage_status: StorageStatus::Ready,
            database_status: DatabaseStatus::Ready,
            logging_status: LoggingStatus::Ready,
            active_task_status: ActiveTaskStatus::None,
            application_version: APPLICATION_VERSION,
            ready: true,
        },
        RuntimePorts {
            repository: RepositoryHandle::new(OperationRiskRepositoryFake {
                lease: ActiveLease {
                    task_id: task.id(),
                    acquired_at_ms: 1,
                },
                project: chatoms_ports::repository::ProjectRecord {
                    id: task.project_id(),
                    name: "fixture".to_owned(),
                    root_path: root_path.clone(),
                    canonical_path_key: root_path.to_lowercase(),
                    display_path: root_path,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
                project_identity: chatoms_ports::repository::ProjectFilesystemIdentityRecord {
                    project_id: task.project_id(),
                    root_volume_serial_hex: "0000000000000001".to_owned(),
                    root_file_id_hex: "00000000000000000000000000000001".to_owned(),
                    repository_kind: chatoms_ports::git::RepositoryKind::Git,
                    git_common_volume_serial_hex: None,
                    git_common_file_id_hex: None,
                    confirmed: true,
                    revision: 1,
                    verified_at_ms: 1,
                },
                task,
                isolation,
                approvals,
                declaration: None,
                persistence_fails,
            }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(GitCapabilityFake {
                available: Ok(true),
            }),
            filesystem,
            worktree_paths: crate::state::WorktreePathHandle::new(
                chatoms_platform::ManagedWorktreePaths::windows_from_environment()
                    .expect("test worktree paths"),
            ),
            provider_capabilities: crate::state::ProviderCapabilityHandle::new(),
            preflight_dir: None,
            planning_runs: crate::state::PlanningRunRegistry::new(),
            implementation_runs: crate::state::ImplementationRunRegistry::new(),
            testing_runs: crate::state::TestingRunRegistry::new(),
            review_runs: crate::state::ReviewRunRegistry::new(),
            merge_abort_runs: crate::state::MergeAbortRunRegistry::new(),
            merge_conflict_writes: crate::state::MergeConflictWriteLock::new(),
        },
        RuntimeResources::default(),
    ))
}

struct MismatchedFilesystemIdentity;

impl chatoms_ports::filesystem::FilesystemIdentityPort for MismatchedFilesystemIdentity {
    fn inspect_supported_directory(
        &mut self,
        path: &std::path::Path,
    ) -> Result<chatoms_ports::filesystem::DirectoryIdentity, PortFailure> {
        Ok(chatoms_ports::filesystem::DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000002".to_owned(),
            file_id_hex: "00000000000000000000000000000002".to_owned(),
        })
    }

    fn verify_local_tree(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Ok(())
    }

    fn acquire_guard(
        &mut self,
        _path: &std::path::Path,
        _expected: &chatoms_ports::filesystem::DirectoryIdentity,
    ) -> Result<Box<dyn chatoms_ports::filesystem::DirectoryIdentityGuard>, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}
