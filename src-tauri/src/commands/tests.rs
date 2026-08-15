use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{Receiver, Sender},
};
use std::thread;
use std::time::Duration;

use chatoms_application::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
    error::ApplicationError,
};
use chatoms_domain::{HighRiskCategory, Task, TaskId, TaskStateTransition};
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, PlatformCapabilityStatus, TimeProvider,
    error::{FailureCategory, PortFailure},
    git::{
        GitService, ProjectInspection, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
    provider::ProviderKind,
    repository::{
        ActiveLease, ContextPackagePreparation, FoundationRepository, HighRiskApprovalRecord,
        ProjectSummary, ProviderBindingRecord, RepositoryError, RepositoryErrorCode,
        TaskGitIsolation, TaskPlanningResultRecord, ValidationCommandApprovalRecord,
    },
};

use super::{
    REGISTERED_HANDLERS, context_package, high_risk_approval, implementation, planning, projects,
    provider_eligibility, review, system, tasks, testing, user_diff_review, validation_commands,
};
use crate::{
    dto::HealthStateDto,
    state::{
        AppRuntime, CapabilityHandle, ManagedRuntime, RepositoryHandle, RuntimePorts,
        RuntimeResources, TimeProviderHandle,
    },
};

#[derive(Default)]
struct CallCounts {
    projects: AtomicUsize,
    active: AtomicUsize,
    task: AtomicUsize,
}

struct RepositoryFake {
    calls: Arc<CallCounts>,
    claude_binding: Option<ProviderBindingRecord>,
    task: Option<Task>,
    planning_result: Option<TaskPlanningResultRecord>,
    review_result: Option<chatoms_ports::repository::TaskReviewResultRecord>,
    isolation: Option<TaskGitIsolation>,
    approvals: Vec<ValidationCommandApprovalRecord>,
    high_risk_approvals: Vec<HighRiskApprovalRecord>,
    /// Scripted result for whichever `prepare_*_context_package` method a
    /// test calls. `None` falls through to `operation_failed()`, matching
    /// every other not-yet-scripted method on this fake.
    prepare_context_package_outcome: Option<Result<ContextPackagePreparation, RepositoryErrorCode>>,
}

impl FoundationRepository for RepositoryFake {
    fn create_task(
        &mut self,
        _task: &Task,
        _initial_transition: &TaskStateTransition,
        _lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        Err(operation_failed())
    }
    fn get_task(&mut self, _task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.calls.task.fetch_add(1, Ordering::SeqCst);
        Ok(self.task.clone())
    }
    fn get_task_planning_result(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskPlanningResultRecord>, RepositoryError> {
        Ok(self.planning_result.clone())
    }
    fn get_task_review_result(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<chatoms_ports::repository::TaskReviewResultRecord>, RepositoryError> {
        Ok(self.review_result.clone())
    }
    fn get_task_brief(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<chatoms_ports::repository::TaskBriefRecord>, RepositoryError> {
        Ok(None)
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
        self.calls.projects.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }
    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.calls.active.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn get_claude_binding(
        &mut self,
        _profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        Ok(self.claude_binding.clone())
    }
    fn get_task_isolation(
        &mut self,
        _task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        Ok(self.isolation.clone())
    }
    fn save_validation_command_approval(
        &mut self,
        approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), RepositoryError> {
        let duplicate = self.approvals.iter().any(|existing| {
            existing.task_id == approval.task_id
                && existing.approved_task_version == approval.approved_task_version
                && existing.kind == approval.kind
        });
        if duplicate {
            return Err(operation_failed());
        }
        self.approvals.push(approval.clone());
        Ok(())
    }
    fn list_validation_command_approvals(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        Ok(self
            .approvals
            .iter()
            .filter(|approval| {
                approval.task_id == task_id
                    && approval.approved_task_version == approved_task_version
            })
            .cloned()
            .collect())
    }
    fn prepare_planning_context_package(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.scripted_context_package_preparation()
    }
    fn prepare_implementation_context_package(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.scripted_context_package_preparation()
    }
    fn prepare_review_context_package(
        &mut self,
        _expected_version: u64,
        _task_id: TaskId,
        _prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.scripted_context_package_preparation()
    }
    /// Derived from `prepare_context_package_outcome` rather than a
    /// dedicated field: a scripted successful preparation already carries
    /// exactly the `ProviderConsent` a readiness check would need to find,
    /// so reusing it keeps this fake minimal. Absent or errored scripting
    /// reports "not found", never an error, matching a plain lookup miss.
    fn get_provider_consent(
        &mut self,
        _task_id: TaskId,
        _provider: ProviderKind,
        _work_kind: chatoms_domain::WorkKind,
        _approved_task_version: u64,
        _data_scope: chatoms_domain::ContextDataScope,
    ) -> Result<Option<chatoms_ports::repository::ProviderConsent>, RepositoryError> {
        Ok(match self.prepare_context_package_outcome {
            Some(Ok(preparation)) => Some(preparation.consent),
            _ => None,
        })
    }
    /// See [`Self::get_provider_consent`]; same derivation from
    /// `prepare_context_package_outcome`.
    fn get_context_package_manifest(
        &mut self,
        _task_id: TaskId,
        _provider: ProviderKind,
        _work_kind: chatoms_domain::WorkKind,
        _approved_task_version: u64,
        _data_scope: chatoms_domain::ContextDataScope,
    ) -> Result<Option<chatoms_ports::repository::ContextPackageManifestRecord>, RepositoryError>
    {
        Ok(match self.prepare_context_package_outcome {
            Some(Ok(preparation)) => Some(preparation.manifest),
            _ => None,
        })
    }
    fn get_high_risk_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        Ok(self
            .high_risk_approvals
            .iter()
            .find(|approval| {
                approval.task_id == task_id
                    && approval.approved_task_version == approved_task_version
                    && approval.risk_category == risk_category
            })
            .copied())
    }
    fn ensure_high_risk_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
        approved_at_ms: i64,
    ) -> Result<HighRiskApprovalRecord, RepositoryError> {
        let task = self
            .task
            .clone()
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if let Some(existing) = self.high_risk_approvals.iter().find(|approval| {
            approval.task_id == task_id
                && approval.approved_task_version == expected_version
                && approval.risk_category == risk_category
        }) {
            return Ok(*existing);
        }
        let approval = HighRiskApprovalRecord {
            task_id,
            approved_task_version: expected_version,
            risk_category,
            approved_at_ms,
        };
        self.high_risk_approvals.push(approval);
        Ok(approval)
    }
}

impl RepositoryFake {
    fn scripted_context_package_preparation(
        &self,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        match self.prepare_context_package_outcome {
            Some(Ok(preparation)) => Ok(preparation),
            Some(Err(code)) => Err(RepositoryError::new(code)),
            None => Err(operation_failed()),
        }
    }
}

fn operation_failed() -> RepositoryError {
    RepositoryError::new(RepositoryErrorCode::OperationFailed)
}

struct TimeFake;

impl TimeProvider for TimeFake {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        Ok(1)
    }
}

struct CapabilityFake;

impl PlatformCapabilityPort for CapabilityFake {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        Ok(PlatformCapabilities {
            secure_storage: PlatformCapabilityStatus::Supported,
            native_permissions: PlatformCapabilityStatus::Supported,
        })
    }
}

struct GitCapabilityFake {
    available: Result<bool, PortFailure>,
}

impl GitService for GitCapabilityFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.available
    }
    fn inspect_project(
        &mut self,
        _input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn repository_status(
        &mut self,
        _root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn validate_non_git_source(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn validate_repository_source(
        &mut self,
        _root: &std::path::Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn initialize_repository(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn has_commit_author(&mut self, _root: &std::path::Path) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn create_initial_snapshot(&mut self, _root: &std::path::Path) -> Result<String, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn create_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn verify_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

struct BlockingGitCapabilityFake {
    started: Sender<()>,
    release: Receiver<()>,
}

impl GitService for BlockingGitCapabilityFake {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        self.started
            .send(())
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        self.release
            .recv()
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        Ok(true)
    }

    fn inspect_project(
        &mut self,
        _input: &std::path::Path,
    ) -> Result<ProjectInspection, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn repository_status(
        &mut self,
        _root: &std::path::Path,
    ) -> Result<RepositoryStatus, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn validate_non_git_source(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn validate_repository_source(
        &mut self,
        _root: &std::path::Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn initialize_repository(&mut self, _root: &std::path::Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn has_commit_author(&mut self, _root: &std::path::Path) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn create_initial_snapshot(&mut self, _root: &std::path::Path) -> Result<String, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn create_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }

    fn verify_task_worktree(
        &mut self,
        _root: &std::path::Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &std::path::Path,
    ) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

fn ready_runtime(calls: Arc<CallCounts>) -> ManagedRuntime {
    ready_runtime_with_git(calls, Ok(true))
}

fn ready_runtime_with_git(
    calls: Arc<CallCounts>,
    available: Result<bool, PortFailure>,
) -> ManagedRuntime {
    ready_runtime_with_git_and_claude_binding(calls, available, None)
}

fn ready_runtime_with_git_and_claude_binding(
    calls: Arc<CallCounts>,
    available: Result<bool, PortFailure>,
    claude_binding: Option<ProviderBindingRecord>,
) -> ManagedRuntime {
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
            repository: RepositoryHandle::new(RepositoryFake {
                calls,
                claude_binding,
                task: None,
                planning_result: None,
                review_result: None,
                isolation: None,
                approvals: Vec::new(),
                high_risk_approvals: Vec::new(),
                prepare_context_package_outcome: None,
            }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(GitCapabilityFake { available }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
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
        },
        RuntimeResources::default(),
    ))
}

fn ready_runtime_with_task(
    calls: Arc<CallCounts>,
    task: Option<Task>,
    planning_result: Option<TaskPlanningResultRecord>,
) -> ManagedRuntime {
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
            repository: RepositoryHandle::new(RepositoryFake {
                calls,
                claude_binding: None,
                task,
                planning_result,
                review_result: None,
                isolation: None,
                approvals: Vec::new(),
                high_risk_approvals: Vec::new(),
                prepare_context_package_outcome: None,
            }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(GitCapabilityFake {
                available: Ok(true),
            }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
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
        },
        RuntimeResources::default(),
    ))
}

/// Mirrors `ready_runtime_with_task`, but seeds a Claude Review result
/// instead of a Claude Planning result — needed for `get_review_result`
/// tests.
fn ready_runtime_with_task_and_review_result(
    calls: Arc<CallCounts>,
    task: Option<Task>,
    review_result: Option<chatoms_ports::repository::TaskReviewResultRecord>,
) -> ManagedRuntime {
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
            repository: RepositoryHandle::new(RepositoryFake {
                calls,
                claude_binding: None,
                task,
                planning_result: None,
                review_result,
                isolation: None,
                approvals: Vec::new(),
                high_risk_approvals: Vec::new(),
                prepare_context_package_outcome: None,
            }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(GitCapabilityFake {
                available: Ok(true),
            }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
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
        },
        RuntimeResources::default(),
    ))
}

/// Mirrors `ready_runtime_with_task`, but additionally seeds an optional
/// `TaskGitIsolation` (Planning's `prepare_planning_context_package`
/// precondition needs one; Implementation's/Review's do not) and a scripted
/// `prepare_*_context_package` outcome — needed for
/// `commands::context_package` tests, none of which spawn a background
/// thread or touch any provider/executable/preflight state.
fn ready_runtime_with_task_isolation_and_context_package_outcome(
    calls: Arc<CallCounts>,
    task: Option<Task>,
    isolation: Option<TaskGitIsolation>,
    prepare_context_package_outcome: Option<Result<ContextPackagePreparation, RepositoryErrorCode>>,
) -> ManagedRuntime {
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
            repository: RepositoryHandle::new(RepositoryFake {
                calls,
                claude_binding: None,
                task,
                planning_result: None,
                review_result: None,
                isolation,
                approvals: Vec::new(),
                high_risk_approvals: Vec::new(),
                prepare_context_package_outcome,
            }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(GitCapabilityFake {
                available: Ok(true),
            }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
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
        },
        RuntimeResources::default(),
    ))
}

/// Mirrors `ready_runtime_with_task`, but with `RuntimeResources.paths`
/// resolved (`Some`) so `AppRuntime::app_temp_dir` succeeds — needed for
/// `start_validation_testing` tests, since (unlike Claude Planning/
/// Implementation, which are gated on a configured executable and preflight
/// directory) Cargo-only Testing execution is gated on a resolved app temp
/// dir instead.
fn ready_runtime_with_task_and_resolved_paths(
    calls: Arc<CallCounts>,
    task: Option<Task>,
) -> ManagedRuntime {
    ready_runtime_with_task_isolation_and_resolved_paths(calls, task, None)
}

/// Mirrors `ready_runtime_with_task_and_resolved_paths`, additionally
/// seeding a `TaskGitIsolation` — needed for
/// `get_validation_command_candidates`/`approve_validation_command` tests,
/// which resolve the task's worktree path via `get_task_isolation`.
fn ready_runtime_with_task_isolation_and_resolved_paths(
    calls: Arc<CallCounts>,
    task: Option<Task>,
    isolation: Option<TaskGitIsolation>,
) -> ManagedRuntime {
    ready_runtime_with_task_isolation_filesystem_and_resolved_paths(
        calls,
        task,
        isolation,
        crate::state::FilesystemIdentityHandle::new(
            chatoms_platform::filesystem::WindowsFilesystemIdentity,
        ),
    )
}

/// Mirrors `ready_runtime_with_task_isolation_and_resolved_paths`, but takes
/// the `FilesystemIdentityHandle` from the caller instead of always using
/// the real Windows adapter — needed by tests that exercise
/// `bind_executable`'s real identity checks (approving a validation
/// command), since the real adapter's cloud/reparse-point rejection is
/// sensitive to the exact machine a test happens to run on.
fn ready_runtime_with_task_isolation_filesystem_and_resolved_paths(
    calls: Arc<CallCounts>,
    task: Option<Task>,
    isolation: Option<TaskGitIsolation>,
    filesystem: crate::state::FilesystemIdentityHandle,
) -> ManagedRuntime {
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
            repository: RepositoryHandle::new(RepositoryFake {
                calls,
                claude_binding: None,
                task,
                planning_result: None,
                review_result: None,
                isolation,
                approvals: Vec::new(),
                high_risk_approvals: Vec::new(),
                prepare_context_package_outcome: None,
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
        },
        RuntimeResources {
            paths: Arc::new(std::sync::Mutex::new(Some(resolved_app_paths_stub()))),
            ..RuntimeResources::default()
        },
    ))
}

fn resolved_app_paths_stub() -> chatoms_ports::path::ResolvedAppPaths {
    chatoms_ports::path::ResolvedAppPaths {
        app_root: std::path::PathBuf::from("C:/chatoms-test/app"),
        data_dir: std::path::PathBuf::from("C:/chatoms-test/app/data"),
        database_path: std::path::PathBuf::from("C:/chatoms-test/app/data/app.db"),
        logs_dir: std::path::PathBuf::from("C:/chatoms-test/app/logs"),
        artifacts_dir: std::path::PathBuf::from("C:/chatoms-test/app/artifacts"),
        temp_dir: std::path::PathBuf::from("C:/chatoms-test/app/temp"),
        worktrees_dir: std::path::PathBuf::from("C:/chatoms-test/app/worktrees"),
    }
}

fn ready_runtime_with_blocking_git(
    calls: Arc<CallCounts>,
    started: Sender<()>,
    release: Receiver<()>,
) -> ManagedRuntime {
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
            repository: RepositoryHandle::new(RepositoryFake {
                calls,
                claude_binding: None,
                task: None,
                planning_result: None,
                review_result: None,
                isolation: None,
                approvals: Vec::new(),
                high_risk_approvals: Vec::new(),
                prepare_context_package_outcome: None,
            }),
            time: TimeProviderHandle::new(TimeFake),
            capabilities: CapabilityHandle::new(CapabilityFake),
            git: crate::state::GitServiceHandle::new(BlockingGitCapabilityFake {
                started,
                release,
            }),
            filesystem: crate::state::FilesystemIdentityHandle::new(
                chatoms_platform::filesystem::WindowsFilesystemIdentity,
            ),
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
        },
        RuntimeResources::default(),
    ))
}

#[test]
fn system_status_exposes_only_the_verified_git_capability_result() {
    let supported = ready_runtime_with_git(Arc::new(CallCounts::default()), Ok(true));
    let supported_status = system::handle_get_system_status(&supported).expect("supported status");
    assert_eq!(
        supported_status.capabilities.git_execution,
        crate::dto::CapabilityStatusDto::Supported
    );
    assert_eq!(
        supported_status.capabilities.claude_execution,
        crate::dto::CapabilityStatusDto::Unavailable
    );
    assert_eq!(
        supported_status.capabilities.codex_execution,
        crate::dto::CapabilityStatusDto::Unavailable
    );
    for unavailable in [
        Ok(false),
        Err(PortFailure::new(FailureCategory::Unsupported)),
    ] {
        let runtime = ready_runtime_with_git(Arc::new(CallCounts::default()), unavailable);
        let status = system::handle_get_system_status(&runtime).expect("unavailable status");
        assert_eq!(
            status.capabilities.git_execution,
            crate::dto::CapabilityStatusDto::Unavailable
        );
        assert_eq!(
            status.capabilities.claude_execution,
            crate::dto::CapabilityStatusDto::Unavailable
        );
        assert_eq!(
            status.capabilities.codex_execution,
            crate::dto::CapabilityStatusDto::Unavailable
        );
        assert!(
            projects::handle_list_projects(&runtime)
                .expect("Git capability does not block project list")
                .is_empty()
        );
    }
}

fn unavailable_runtime() -> ManagedRuntime {
    let category = FailureCategory::StorageUnavailable;
    ManagedRuntime::unavailable(
        ApplicationError::from_failure(
            category,
            category.default_severity(),
            category.default_retry(),
        ),
        None,
    )
}

#[test]
fn ready_system_and_empty_foundation_commands_use_services_once() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls.clone());
    assert_eq!(
        system::handle_get_version(&runtime)
            .expect("version")
            .version,
        "0.1.0"
    );
    assert_eq!(
        system::handle_get_health(&runtime).expect("health").status,
        HealthStateDto::Healthy
    );
    assert!(
        projects::handle_list_projects(&runtime)
            .expect("projects")
            .is_empty()
    );
    assert!(
        tasks::handle_get_active_task(&runtime)
            .expect("active task")
            .is_none()
    );
    assert_eq!(calls.projects.load(Ordering::SeqCst), 1);
    assert_eq!(calls.active.load(Ordering::SeqCst), 1);
}

#[test]
fn read_only_project_list_remains_available_while_git_capability_probe_is_running() {
    let (started_send, started_receive) = std::sync::mpsc::channel();
    let (release_send, release_receive) = std::sync::mpsc::channel();
    let calls = Arc::new(CallCounts::default());
    let runtime = Arc::new(ready_runtime_with_blocking_git(
        calls.clone(),
        started_send,
        release_receive,
    ));
    let probing_runtime = runtime.clone();
    let probe = thread::spawn(move || system::handle_get_system_status(&probing_runtime));

    started_receive
        .recv()
        .expect("Git capability probe started");
    let (list_send, list_receive) = std::sync::mpsc::channel();
    let list_runtime = runtime.clone();
    let list = thread::spawn(move || list_send.send(projects::handle_list_projects(&list_runtime)));
    let list_result = list_receive.recv_timeout(Duration::from_secs(1));

    release_send.send(()).expect("release Git capability probe");
    assert_eq!(
        probe
            .join()
            .expect("capability probe thread")
            .expect("system status")
            .capabilities
            .git_execution,
        crate::dto::CapabilityStatusDto::Supported
    );
    list.join()
        .expect("project list thread")
        .expect("project list result sent");
    assert!(
        list_result
            .expect("project list must not wait for a Git capability probe")
            .expect("project list remains read-only available")
            .is_empty()
    );
    assert_eq!(calls.projects.load(Ordering::SeqCst), 1);
}

#[test]
fn task_not_found_and_unavailable_state_return_stable_safe_errors() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls.clone());
    let task_id = TaskId::new().to_string();
    let error = tasks::handle_get_task(&runtime, &task_id).expect_err("missing task");
    assert_eq!(error.code, "APP_NOT_FOUND");
    assert_eq!(calls.task.load(Ordering::SeqCst), 1);
    assert!(!error.to_string().contains("SELECT"));
    assert!(!error.to_string().contains("C:\\"));

    let error = provider_eligibility::handle_get_provider_eligibility(&runtime, &task_id)
        .expect_err("missing task eligibility");
    assert_eq!(error.code, "APP_NOT_FOUND");
    assert_eq!(calls.task.load(Ordering::SeqCst), 2);
    assert!(!error.to_string().contains("SELECT"));

    let runtime = unavailable_runtime();
    assert_eq!(
        system::handle_get_health(&runtime)
            .expect("safe health")
            .status,
        HealthStateDto::Unavailable
    );
    for error in [
        projects::handle_list_projects(&runtime).expect_err("projects unavailable"),
        tasks::handle_get_active_task(&runtime).expect_err("tasks unavailable"),
    ] {
        assert_eq!(error.code, "APP_STORAGE_UNAVAILABLE");
        assert_eq!(error.message, "Secure local storage is unavailable.");
    }
}

#[test]
fn handler_allowlist_contains_only_approved_purpose_specific_commands() {
    assert_eq!(REGISTERED_HANDLERS.len(), 45);
    assert_eq!(
        REGISTERED_HANDLERS,
        [
            "get_version",
            "get_health",
            "get_system_status",
            "get_bootstrap_status",
            "get_legacy_migration_diagnostic",
            "list_projects",
            "inspect_project_candidate",
            "register_project",
            "get_project_git_status",
            "create_isolation_task",
            "get_task_isolation",
            "approve_git_initialization",
            "create_task_worktree",
            "get_active_task",
            "get_task",
            "list_task_history",
            "get_provider_eligibility",
            "set_claude_executable_path",
            "refresh_claude_capability",
            "start_claude_planning",
            "cancel_claude_planning",
            "get_planning_result",
            "get_context_package_planning_readiness",
            "start_claude_planning_context_package",
            "start_claude_implementation",
            "cancel_claude_implementation",
            "get_context_package_implementation_readiness",
            "start_claude_implementation_context_package",
            "start_validation_testing",
            "cancel_validation_testing",
            "get_validation_command_candidates",
            "get_validation_command_approval_status",
            "approve_validation_command",
            "start_claude_review",
            "cancel_claude_review",
            "get_review_result",
            "prepare_planning_context_package",
            "prepare_implementation_context_package",
            "prepare_review_context_package",
            "get_context_package_review_readiness",
            "start_claude_review_context_package",
            "get_high_risk_approval_status",
            "approve_high_risk_operation",
            "get_user_diff_for_review",
            "approve_user_diff",
        ]
    );
    for forbidden in [
        "create_task",
        "transition_task",
        "run_shell",
        "git",
        "updater",
        "installer",
        "credentials",
    ] {
        assert!(!REGISTERED_HANDLERS.contains(&forbidden));
    }
}

#[test]
fn provider_capability_handle_generation_and_cache_invariants() {
    use crate::state::{CachedProviderCapabilities, ProviderCapabilityHandle, RefreshOutcome};
    use chatoms_application::system::CapabilityStatus as AppCapabilityStatus;

    let handle = ProviderCapabilityHandle::new();
    assert_eq!(handle.generation(), 0);
    let cached = handle.read_cache();
    assert_eq!(cached.claude, None);
    assert_eq!(cached.codex, None);

    handle.invalidate_and_bump_generation();
    assert_eq!(handle.generation(), 1);
    let cached = handle.read_cache();
    assert_eq!(cached.claude, None);

    let g = handle.try_begin_refresh().expect("begin refresh");
    assert_eq!(g, 1);
    let result = handle.finish_refresh(
        g,
        CachedProviderCapabilities {
            claude: Some(AppCapabilityStatus::Supported),
            codex: Some(AppCapabilityStatus::Unsupported),
        },
    );
    assert_eq!(result, RefreshOutcome::Completed);
    let cached = handle.read_cache();
    assert_eq!(cached.claude, Some(AppCapabilityStatus::Supported));
    assert_eq!(cached.codex, Some(AppCapabilityStatus::Unsupported));
}

#[test]
fn stale_refresh_returns_superseded_and_does_not_overwrite_cache() {
    use crate::state::{CachedProviderCapabilities, ProviderCapabilityHandle, RefreshOutcome};
    use chatoms_application::system::CapabilityStatus as AppCapabilityStatus;

    let handle = ProviderCapabilityHandle::new();
    let g = handle.try_begin_refresh().expect("begin refresh");

    handle.invalidate_and_bump_generation();
    assert_eq!(handle.generation(), 1);

    let result = handle.finish_refresh(
        g,
        CachedProviderCapabilities {
            claude: Some(AppCapabilityStatus::Supported),
            codex: Some(AppCapabilityStatus::Unsupported),
        },
    );
    assert_eq!(result, RefreshOutcome::Superseded);
    let cached = handle.read_cache();
    assert_eq!(
        cached.claude, None,
        "old Supported must not survive a generation change"
    );
    assert_eq!(cached.codex, None);
}

#[test]
fn concurrent_refresh_returns_conflict_without_starting_second_probe() {
    use crate::state::ProviderCapabilityHandle;

    let handle = ProviderCapabilityHandle::new();
    let _gen = handle.try_begin_refresh().expect("first refresh begins");
    assert!(
        handle.try_begin_refresh().is_none(),
        "second concurrent refresh must be rejected"
    );
    handle.abort_refresh();
    assert!(
        handle.try_begin_refresh().is_some(),
        "refresh available after abort"
    );
}

#[test]
fn get_system_status_does_not_run_provider_probe() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls.clone());
    let status = system::handle_get_system_status(&runtime).expect("system status");
    assert_eq!(
        status.capabilities.claude_execution,
        crate::dto::CapabilityStatusDto::Unavailable,
        "system status must report cache only, never run a provider probe"
    );
    assert_eq!(
        status.capabilities.codex_execution,
        crate::dto::CapabilityStatusDto::Unavailable,
    );
}

#[test]
fn refresh_during_system_status_does_not_block_project_list() {
    let (started_send, started_receive) = std::sync::mpsc::channel();
    let (release_send, release_receive) = std::sync::mpsc::channel();
    let calls = Arc::new(CallCounts::default());
    let runtime = Arc::new(ready_runtime_with_blocking_git(
        calls.clone(),
        started_send,
        release_receive,
    ));

    let probing_runtime = runtime.clone();
    let probe = thread::spawn(move || system::handle_get_system_status(&probing_runtime));

    started_receive.recv().expect("probe started");

    let list_runtime = runtime.clone();
    let (list_send, list_receive) = std::sync::mpsc::channel();
    let list = thread::spawn(move || list_send.send(projects::handle_list_projects(&list_runtime)));
    let list_result = list_receive.recv_timeout(Duration::from_secs(1));

    release_send.send(()).expect("release probe");
    probe.join().expect("probe thread").expect("system status");
    list.join().expect("list thread").expect("list result");
    assert!(
        list_result
            .expect("project list must not block on git probe")
            .expect("project list available")
            .is_empty()
    );
}

#[test]
fn provider_dto_serialization_is_camel_case_and_path_free() {
    use crate::dto::{
        CapabilityStatusDto, RefreshClaudeCapabilityDto, RefreshOutcomeDto,
        SetClaudeExecutablePathDto,
    };
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    let set_response = SetClaudeExecutablePathDto {
        display_path: "%USERPROFILE%\\AppData\\claude.exe".to_owned(),
        claude_execution: CapabilityStatusDto::Unavailable,
    };
    let InvokeResponseBody::Json(json) = set_response.body().expect("serialized") else {
        panic!("expected JSON");
    };
    assert!(json.contains("\"displayPath\":\"%USERPROFILE%"));
    assert!(json.contains("\"claudeExecution\":\"unavailable\""));
    assert!(!json.contains("C:\\\\"));

    let refresh_response = RefreshClaudeCapabilityDto {
        outcome: RefreshOutcomeDto::Completed,
        claude_execution: CapabilityStatusDto::Supported,
        codex_execution: CapabilityStatusDto::Unsupported,
    };
    let InvokeResponseBody::Json(json) = refresh_response.body().expect("serialized") else {
        panic!("expected JSON");
    };
    assert!(json.contains("\"outcome\":\"completed\""));
    assert!(json.contains("\"claudeExecution\":\"supported\""));
    assert!(json.contains("\"codexExecution\":\"unsupported\""));
}

#[test]
fn generation_only_increments_after_invalidate_not_on_read() {
    use crate::state::ProviderCapabilityHandle;

    let handle = ProviderCapabilityHandle::new();
    assert_eq!(handle.generation(), 0);
    handle.read_cache();
    assert_eq!(handle.generation(), 0, "read must not change generation");
    handle.invalidate_and_bump_generation();
    assert_eq!(handle.generation(), 1);
    handle.invalidate_and_bump_generation();
    assert_eq!(handle.generation(), 2);
}

#[test]
fn start_claude_planning_without_a_configured_executable_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), None);

    let error = planning::handle_start_claude_planning(&runtime, &TaskId::new().to_string(), 1)
        .expect_err("no executable path is configured");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_planning_without_a_preflight_directory_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:\\trusted\\claude.exe".to_owned()),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), Some(binding));

    let error = planning::handle_start_claude_planning(&runtime, &TaskId::new().to_string(), 1)
        .expect_err("no preflight directory is available in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_planning_context_package_without_a_configured_executable_is_unsupported_and_starts_nothing()
 {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), None);

    let error = planning::handle_start_claude_planning_context_package(
        &runtime,
        &TaskId::new().to_string(),
        1,
    )
    .expect_err("no executable path is configured");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_planning_context_package_without_a_preflight_directory_is_unsupported_and_starts_nothing()
 {
    let calls = Arc::new(CallCounts::default());
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:\\trusted\\claude.exe".to_owned()),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), Some(binding));

    let error = planning::handle_start_claude_planning_context_package(
        &runtime,
        &TaskId::new().to_string(),
        1,
    )
    .expect_err("no preflight directory is available in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn cancel_claude_planning_reports_whether_a_matching_run_was_found() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls);
    let task_id = TaskId::new();

    let none_found = planning::handle_cancel_claude_planning(&runtime, &task_id.to_string())
        .expect("cancel never fails, even with nothing to cancel");
    assert!(!none_found.requested);

    let ready = runtime.ready_snapshot().expect("ready runtime");
    let _signal = ready
        .planning_runs
        .register(task_id)
        .expect("first registration for this task id");
    let found = planning::handle_cancel_claude_planning(&runtime, &task_id.to_string())
        .expect("cancel a registered run");
    assert!(found.requested);
}

#[test]
fn start_claude_implementation_without_a_configured_executable_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), None);

    let error =
        implementation::handle_start_claude_implementation(&runtime, &TaskId::new().to_string(), 1)
            .expect_err("no executable path is configured");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_implementation_without_a_preflight_directory_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:\\trusted\\claude.exe".to_owned()),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), Some(binding));

    let error =
        implementation::handle_start_claude_implementation(&runtime, &TaskId::new().to_string(), 1)
            .expect_err("no preflight directory is available in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_implementation_context_package_without_a_configured_executable_is_unsupported_and_starts_nothing()
 {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), None);

    let error = implementation::handle_start_claude_implementation_context_package(
        &runtime,
        &TaskId::new().to_string(),
        1,
    )
    .expect_err("no executable path is configured");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_implementation_context_package_without_a_preflight_directory_is_unsupported_and_starts_nothing()
 {
    let calls = Arc::new(CallCounts::default());
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:\\trusted\\claude.exe".to_owned()),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), Some(binding));

    let error = implementation::handle_start_claude_implementation_context_package(
        &runtime,
        &TaskId::new().to_string(),
        1,
    )
    .expect_err("no preflight directory is available in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn get_context_package_implementation_readiness_is_true_when_the_pair_is_prepared() {
    let task_id = TaskId::new();
    let preparation =
        context_package_preparation(task_id, chatoms_domain::WorkKind::Implementation, 1);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        None,
        None,
        Some(Ok(preparation)),
    );

    let dto = context_package::handle_get_context_package_implementation_readiness(
        &runtime,
        &task_id.to_string(),
        1,
    )
    .expect("readiness lookup succeeds");

    assert!(dto.ready);
}

#[test]
fn get_context_package_implementation_readiness_is_false_when_nothing_is_prepared() {
    let task_id = TaskId::new();
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        None,
        None,
        None,
    );

    let dto = context_package::handle_get_context_package_implementation_readiness(
        &runtime,
        &task_id.to_string(),
        1,
    )
    .expect("readiness lookup succeeds");

    assert!(!dto.ready);
}

#[test]
fn cancel_claude_implementation_reports_whether_a_matching_run_was_found() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls);
    let task_id = TaskId::new();

    let none_found =
        implementation::handle_cancel_claude_implementation(&runtime, &task_id.to_string())
            .expect("cancel never fails, even with nothing to cancel");
    assert!(!none_found.requested);

    let ready = runtime.ready_snapshot().expect("ready runtime");
    let _signal = ready
        .implementation_runs
        .register(task_id)
        .expect("first registration for this task id");
    let found = implementation::handle_cancel_claude_implementation(&runtime, &task_id.to_string())
        .expect("cancel a registered run");
    assert!(found.requested);
}

#[test]
fn start_validation_testing_without_a_resolved_app_temp_dir_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls.clone());

    let error = testing::handle_start_validation_testing(&runtime, &TaskId::new().to_string(), 1)
        .expect_err("no resolved app paths are configured in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once the app temp dir is unavailable"
    );
}

#[test]
fn start_validation_testing_propagates_a_starter_rejection_without_registering_or_transitioning() {
    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let runtime = ready_runtime_with_task_and_resolved_paths(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
    );

    let error =
        testing::handle_start_validation_testing(&runtime, &task.id().to_string(), task.version())
            .expect_err("this fake repository has no isolation record, so begin() must reject");

    assert_eq!(error.code, "APP_NOT_FOUND");
    let ready = runtime.ready_snapshot().expect("ready runtime");
    assert!(
        !ready.testing_runs.request_cancellation(task.id()),
        "no run may ever be registered when TestingBatchStarter::begin rejects"
    );
}

#[test]
fn cancel_validation_testing_reports_whether_a_matching_run_was_found() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls);
    let task_id = TaskId::new();

    let none_found = testing::handle_cancel_validation_testing(&runtime, &task_id.to_string())
        .expect("cancel never fails, even with nothing to cancel");
    assert!(!none_found.requested);

    let ready = runtime.ready_snapshot().expect("ready runtime");
    let _signal = ready
        .testing_runs
        .register(task_id)
        .expect("first registration for this task id");
    let found = testing::handle_cancel_validation_testing(&runtime, &task_id.to_string())
        .expect("cancel a registered run");
    assert!(found.requested);
}

/// A real temp directory used only so `ManifestValidationCommandDiscovery`
/// (the same production discovery adapter the commands under test always
/// use) can read manifest *presence* — never executed, never given a real
/// build. Mirrors `chatoms_infrastructure::validation_discovery`'s own test
/// fixture.
struct TempWorktree {
    path: std::path::PathBuf,
}

impl TempWorktree {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "chatoms-validation-commands-ipc-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp worktree");
        Self { path }
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.path.join(name), contents).expect("write manifest fixture");
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A `FilesystemIdentityPort` that echoes every queried path back as its own
/// canonical path with one fixed identity. Used only where a real
/// approval's exact identity fields do not matter (a fresh approval, never
/// re-verified against a prior one within the same test) — the real Windows
/// adapter is deliberately not used here because its cloud/reparse-point
/// rejection depends on the exact machine and profile a test happens to run
/// under, exactly as `chatoms_infrastructure::validation_execution`'s own
/// tests avoid it for the same reason.
#[derive(Clone, Copy, Default)]
struct EchoFilesystemIdentity;

impl chatoms_ports::filesystem::FilesystemIdentityPort for EchoFilesystemIdentity {
    fn inspect_supported_directory(
        &mut self,
        path: &std::path::Path,
    ) -> Result<chatoms_ports::filesystem::DirectoryIdentity, PortFailure> {
        Ok(chatoms_ports::filesystem::DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "0000000000000001".to_owned(),
            file_id_hex: "00000000000000000000000000000001".to_owned(),
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

    fn inspect_supported_file(
        &mut self,
        path: &std::path::Path,
    ) -> Result<chatoms_ports::filesystem::DirectoryIdentity, PortFailure> {
        self.inspect_supported_directory(path)
    }
}

fn worktree_ready_isolation(
    task_id: TaskId,
    project_id: chatoms_domain::ProjectId,
    worktree_path: &std::path::Path,
    expected_task_version: u64,
) -> TaskGitIsolation {
    TaskGitIsolation {
        task_id,
        project_id,
        status: chatoms_ports::repository::GitIsolationStatus::WorktreeReady,
        operation_id: None,
        expected_task_version,
        base_branch: Some("main".to_owned()),
        base_commit: Some("abc123".to_owned()),
        worktree_path: Some(worktree_path.to_string_lossy().into_owned()),
        branch_created_by_app: true,
        worktree_created_by_app: true,
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

#[test]
fn get_validation_command_candidates_returns_only_cargo_fixed_candidates_with_safe_labels() {
    let worktree = TempWorktree::new("candidates");
    worktree.write("Cargo.toml", "[package]\nname = \"fixture\"\n");
    worktree.write(
        "package.json",
        r#"{"scripts": {"test": "vitest", "lint": "eslint ."}}"#,
    );

    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let isolation =
        worktree_ready_isolation(task.id(), task.project_id(), &worktree.path, task.version());
    let runtime = ready_runtime_with_task_isolation_and_resolved_paths(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(isolation),
    );

    let candidates = validation_commands::handle_get_validation_command_candidates(
        &runtime,
        &task.id().to_string(),
    )
    .expect("candidates load");

    assert_eq!(
        candidates.len(),
        4,
        "package.json's npm-based candidates must never leak into this Cargo-only surface"
    );
    let kinds: std::collections::HashSet<_> = candidates.iter().map(|c| c.kind).collect();
    for expected in [
        crate::dto::ValidationCommandKindDto::Format,
        crate::dto::ValidationCommandKindDto::Lint,
        crate::dto::ValidationCommandKindDto::Test,
        crate::dto::ValidationCommandKindDto::Build,
    ] {
        assert!(kinds.contains(&expected), "missing {expected:?}");
    }
    assert!(
        !kinds.contains(&crate::dto::ValidationCommandKindDto::Typecheck),
        "Cargo never proposes a Typecheck candidate"
    );
    for candidate in &candidates {
        assert!(!candidate.label.is_empty());
    }
}

#[test]
fn get_validation_command_approval_status_reports_only_approved_kinds() {
    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let status = validation_commands::handle_get_validation_command_approval_status(
        &runtime,
        &task.id().to_string(),
    )
    .expect("approval status loads even with nothing approved yet");
    assert!(status.approved_kinds.is_empty());
}

#[test]
fn approve_validation_command_rejects_empty_selection_duplicate_kind_and_blank_executable_path() {
    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);
    let executable = std::env::current_exe()
        .expect("test executable path")
        .to_string_lossy()
        .into_owned();

    let empty_selection = validation_commands::handle_approve_validation_command(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::ApproveValidationCommandInputDto {
            kinds: Vec::new(),
            executable_path: executable.clone(),
            cargo_home_path: None,
            rustup_home_path: None,
        },
    )
    .expect_err("an empty selection must be rejected at the boundary");
    assert_eq!(empty_selection.code, "APP_INVALID_INPUT");

    let duplicate_kind = validation_commands::handle_approve_validation_command(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::ApproveValidationCommandInputDto {
            kinds: vec![
                crate::dto::ValidationCommandKindDto::Test,
                crate::dto::ValidationCommandKindDto::Test,
            ],
            executable_path: executable.clone(),
            cargo_home_path: None,
            rustup_home_path: None,
        },
    )
    .expect_err("a duplicate kind must be rejected at the boundary");
    assert_eq!(duplicate_kind.code, "APP_INVALID_INPUT");

    let blank_executable = validation_commands::handle_approve_validation_command(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::ApproveValidationCommandInputDto {
            kinds: vec![crate::dto::ValidationCommandKindDto::Test],
            executable_path: "   ".to_owned(),
            cargo_home_path: None,
            rustup_home_path: None,
        },
    )
    .expect_err("a blank executable path must be rejected at the boundary");
    assert_eq!(blank_executable.code, "APP_INVALID_INPUT");

    let status = validation_commands::handle_get_validation_command_approval_status(
        &runtime,
        &task.id().to_string(),
    )
    .expect("approval status loads");
    assert!(
        status.approved_kinds.is_empty(),
        "no boundary-rejected request may write anything"
    );
}

#[test]
fn approve_validation_command_uses_the_current_discovery_candidate_not_a_frontend_supplied_argv() {
    let worktree = TempWorktree::new("approve");
    worktree.write("Cargo.toml", "[package]\nname = \"fixture\"\n");

    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let isolation =
        worktree_ready_isolation(task.id(), task.project_id(), &worktree.path, task.version());
    let runtime = ready_runtime_with_task_isolation_filesystem_and_resolved_paths(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(isolation),
        crate::state::FilesystemIdentityHandle::new(EchoFilesystemIdentity),
    );

    let result = validation_commands::handle_approve_validation_command(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::ApproveValidationCommandInputDto {
            kinds: vec![crate::dto::ValidationCommandKindDto::Test],
            executable_path: "C:/fake-tools/cargo/bin/cargo.exe".to_owned(),
            cargo_home_path: None,
            rustup_home_path: None,
        },
    )
    .expect("approving a kind with a real current Cargo candidate succeeds");
    assert_eq!(
        result.approved_kinds,
        vec![crate::dto::ValidationCommandKindDto::Test]
    );

    let status = validation_commands::handle_get_validation_command_approval_status(
        &runtime,
        &task.id().to_string(),
    )
    .expect("approval status loads");
    assert_eq!(
        status.approved_kinds,
        vec![crate::dto::ValidationCommandKindDto::Test]
    );
}

#[test]
fn approve_validation_command_rejects_a_kind_with_no_current_cargo_candidate_and_writes_nothing() {
    // No Cargo.toml at all: discovery proposes no Cargo candidates, so every
    // kind is unresolvable.
    let worktree = TempWorktree::new("no-cargo-manifest");

    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let isolation =
        worktree_ready_isolation(task.id(), task.project_id(), &worktree.path, task.version());
    let runtime = ready_runtime_with_task_isolation_and_resolved_paths(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(isolation),
    );
    let executable = std::env::current_exe()
        .expect("test executable path")
        .to_string_lossy()
        .into_owned();

    let error = validation_commands::handle_approve_validation_command(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::ApproveValidationCommandInputDto {
            kinds: vec![crate::dto::ValidationCommandKindDto::Test],
            executable_path: executable,
            cargo_home_path: None,
            rustup_home_path: None,
        },
    )
    .expect_err("no Cargo candidate currently matches the selected kind");
    assert_eq!(error.code, "APP_INVALID_INPUT");

    let status = validation_commands::handle_get_validation_command_approval_status(
        &runtime,
        &task.id().to_string(),
    )
    .expect("approval status loads");
    assert!(
        status.approved_kinds.is_empty(),
        "a request where any selected kind fails to resolve must approve nothing"
    );
}

fn task_in_state(state: chatoms_domain::TaskState) -> Task {
    use chatoms_domain::{TaskBranchIdentity, TaskSnapshot};
    let id = TaskId::new();
    Task::restore(TaskSnapshot {
        id,
        project_id: chatoms_domain::ProjectId::new(),
        state,
        version: 1,
        task_branch_identity: TaskBranchIdentity::for_task(id),
        resume_target_state: None,
        created_at_ms: 10,
        updated_at_ms: 10,
        terminal_at_ms: None,
    })
    .expect("test task must satisfy domain invariants")
}

fn planning_result_record(task_id: TaskId, plan_text: &str) -> TaskPlanningResultRecord {
    TaskPlanningResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: chatoms_domain::WorkKind::Planning,
        outcome: chatoms_ports::repository::PlanningResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(2),
        started_at_ms: 10,
        completed_at_ms: 20,
        plan_text: Some(plan_text.to_owned()),
    }
}

#[test]
fn get_planning_result_returns_the_stored_result_only_in_awaiting_design_approval() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let record = planning_result_record(task.id(), "Add a CSV export button.");
    let runtime = ready_runtime_with_task(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(record),
    );

    let result = planning::handle_get_planning_result(&runtime, &task.id().to_string())
        .expect("planning result lookup succeeds")
        .expect("a result was recorded for this task");
    assert_eq!(
        result.plan_text.as_deref(),
        Some("Add a CSV export button.")
    );
    assert_eq!(result.outcome, crate::dto::PlanningOutcomeDto::Completed);
}

#[test]
fn get_planning_result_is_hidden_outside_awaiting_design_approval() {
    let task = task_in_state(chatoms_domain::TaskState::Planning);
    let record = planning_result_record(task.id(), "Should never surface.");
    let runtime = ready_runtime_with_task(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(record),
    );

    let result = planning::handle_get_planning_result(&runtime, &task.id().to_string())
        .expect("planning result lookup succeeds");
    assert!(
        result.is_none(),
        "a task outside AwaitingDesignApproval must never expose its planning result"
    );
}

#[test]
fn get_planning_result_reports_no_result_when_none_is_recorded_yet() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let result = planning::handle_get_planning_result(&runtime, &task.id().to_string())
        .expect("planning result lookup succeeds");
    assert!(result.is_none());
}

#[test]
fn get_planning_result_for_a_missing_task_is_a_safe_not_found_error() {
    let runtime = ready_runtime_with_task(Arc::new(CallCounts::default()), None, None);

    let error = planning::handle_get_planning_result(&runtime, &TaskId::new().to_string())
        .expect_err("missing task");
    assert_eq!(error.code, "APP_NOT_FOUND");
}

#[test]
fn start_claude_review_without_a_configured_executable_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), None);

    let error = review::handle_start_claude_review(&runtime, &TaskId::new().to_string(), 1)
        .expect_err("no executable path is configured");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
    let ready = runtime.ready_snapshot().expect("ready runtime");
    assert!(
        !ready.review_runs.request_cancellation(TaskId::new()),
        "no run may ever be registered when the capability gate rejects"
    );
}

#[test]
fn start_claude_review_without_a_preflight_directory_is_unsupported_and_starts_nothing() {
    let calls = Arc::new(CallCounts::default());
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:\\trusted\\claude.exe".to_owned()),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), Some(binding));

    let error = review::handle_start_claude_review(&runtime, &TaskId::new().to_string(), 1)
        .expect_err("no preflight directory is available in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn start_claude_review_context_package_without_a_configured_executable_is_unsupported_and_starts_nothing()
 {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), None);

    let error =
        review::handle_start_claude_review_context_package(&runtime, &TaskId::new().to_string(), 1)
            .expect_err("no executable path is configured");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
    let ready = runtime.ready_snapshot().expect("ready runtime");
    assert!(
        !ready.review_runs.request_cancellation(TaskId::new()),
        "no run may ever be registered when the capability gate rejects"
    );
}

#[test]
fn start_claude_review_context_package_without_a_preflight_directory_is_unsupported_and_starts_nothing()
 {
    let calls = Arc::new(CallCounts::default());
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:\\trusted\\claude.exe".to_owned()),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let runtime = ready_runtime_with_git_and_claude_binding(calls.clone(), Ok(true), Some(binding));

    let error =
        review::handle_start_claude_review_context_package(&runtime, &TaskId::new().to_string(), 1)
            .expect_err("no preflight directory is available in this test runtime");

    assert_eq!(error.code, "APP_UNSUPPORTED");
    assert_eq!(
        calls.task.load(Ordering::SeqCst),
        0,
        "the task must never be loaded once capability is rejected"
    );
}

#[test]
fn get_context_package_review_readiness_is_true_when_the_pair_is_prepared() {
    let task_id = TaskId::new();
    let preparation = context_package_preparation(task_id, chatoms_domain::WorkKind::Review, 1);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        None,
        None,
        Some(Ok(preparation)),
    );

    let dto = context_package::handle_get_context_package_review_readiness(
        &runtime,
        &task_id.to_string(),
        1,
    )
    .expect("readiness lookup succeeds");

    assert!(dto.ready);
}

#[test]
fn get_context_package_review_readiness_is_false_when_nothing_is_prepared() {
    let task_id = TaskId::new();
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        None,
        None,
        None,
    );

    let dto = context_package::handle_get_context_package_review_readiness(
        &runtime,
        &task_id.to_string(),
        1,
    )
    .expect("readiness lookup succeeds");

    assert!(!dto.ready);
}

const ALL_HIGH_RISK_CATEGORY_DTOS: [crate::dto::HighRiskCategoryDto; 13] = [
    crate::dto::HighRiskCategoryDto::ArchitectureChange,
    crate::dto::HighRiskCategoryDto::DatabaseSchemaChange,
    crate::dto::HighRiskCategoryDto::AuthenticationOrAuthorizationChange,
    crate::dto::HighRiskCategoryDto::SecurityPolicyChange,
    crate::dto::HighRiskCategoryDto::ExternalNetworkBehaviorAddition,
    crate::dto::HighRiskCategoryDto::ExternalDataTransmissionAddition,
    crate::dto::HighRiskCategoryDto::LargeScaleFileMoveOrDeletion,
    crate::dto::HighRiskCategoryDto::PublicApiOrStorageFormatChange,
    crate::dto::HighRiskCategoryDto::OperatingSystemConfigurationChange,
    crate::dto::HighRiskCategoryDto::AdministratorPrivilegesRequired,
    crate::dto::HighRiskCategoryDto::BreakingCompatibilityChange,
    crate::dto::HighRiskCategoryDto::DataMigration,
    crate::dto::HighRiskCategoryDto::DifficultToRecoverChange,
];

#[test]
fn get_high_risk_approval_status_is_false_when_nothing_is_approved() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let status = high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect("status lookup succeeds");

    assert!(!status.approved);
}

#[test]
fn get_high_risk_approval_status_for_a_missing_task_is_a_safe_not_found_error() {
    let runtime = ready_runtime_with_task(Arc::new(CallCounts::default()), None, None);

    let error = high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &TaskId::new().to_string(),
        1,
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect_err("missing task");

    assert_eq!(error.code, "APP_NOT_FOUND");
}

#[test]
fn get_high_risk_approval_status_propagates_a_stale_version_error_instead_of_false() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let error = high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &task.id().to_string(),
        task.version() + 1,
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect_err("a stale expected_version must be rejected, never reported as approved: false");

    assert_eq!(error.code, "APP_VERSION_CONFLICT");
}

#[test]
fn approve_high_risk_operation_first_call_creates_the_approval_and_status_then_reports_it() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let approval = high_risk_approval::handle_approve_high_risk_operation(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect("first approval succeeds");
    assert_eq!(
        approval.risk_category,
        crate::dto::HighRiskCategoryDto::DataMigration
    );

    let status = high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect("status lookup succeeds");
    assert!(status.approved);

    // A different category for the same task/version must remain unapproved
    // -- approving one category must never mark another as approved.
    let other = high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::ArchitectureChange,
    )
    .expect("status lookup succeeds");
    assert!(!other.approved);
}

#[test]
fn approve_high_risk_operation_reuses_the_existing_approval_with_identical_success_semantics() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let first = high_risk_approval::handle_approve_high_risk_operation(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::BreakingCompatibilityChange,
    )
    .expect("first call creates the approval");
    let second = high_risk_approval::handle_approve_high_risk_operation(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::BreakingCompatibilityChange,
    )
    .expect("second call must reuse, not fail");

    assert_eq!(
        first, second,
        "create and reuse must return the same content-free success result"
    );
}

#[test]
fn approve_high_risk_operation_rejects_a_stale_version_and_creates_nothing() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    let error = high_risk_approval::handle_approve_high_risk_operation(
        &runtime,
        &task.id().to_string(),
        task.version() + 1,
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect_err("a stale expected_version must be rejected");
    assert_eq!(error.code, "APP_VERSION_CONFLICT");

    let status = high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect("status lookup succeeds");
    assert!(
        !status.approved,
        "a rejected stale-version approve must never leave an approval behind"
    );
}

#[test]
fn approve_high_risk_operation_for_a_missing_task_is_a_safe_not_found_error() {
    let runtime = ready_runtime_with_task(Arc::new(CallCounts::default()), None, None);

    let error = high_risk_approval::handle_approve_high_risk_operation(
        &runtime,
        &TaskId::new().to_string(),
        1,
        crate::dto::HighRiskCategoryDto::DataMigration,
    )
    .expect_err("missing task");

    assert_eq!(error.code, "APP_NOT_FOUND");
}

#[test]
fn approve_high_risk_operation_never_changes_task_state_version_history_or_lease() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime_with_task(Arc::clone(&calls), Some(task.clone()), None);

    // `RepositoryFake::save_transition`/`terminate_task`/`save_recovery_target`
    // all unconditionally return `Err(operation_failed())`; if approving
    // (or checking) a high-risk category ever drove any of those, this
    // whole test would fail outright rather than merely leaving stale
    // assertions unexercised.
    high_risk_approval::handle_approve_high_risk_operation(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::SecurityPolicyChange,
    )
    .expect("approve succeeds");
    high_risk_approval::handle_get_high_risk_approval_status(
        &runtime,
        &task.id().to_string(),
        task.version(),
        crate::dto::HighRiskCategoryDto::SecurityPolicyChange,
    )
    .expect("status lookup succeeds");

    assert_eq!(
        calls.active.load(Ordering::SeqCst),
        0,
        "neither approve nor status ever queries the active lease"
    );
}

#[test]
fn high_risk_approval_status_and_approve_round_trip_for_all_thirteen_categories() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime =
        ready_runtime_with_task(Arc::new(CallCounts::default()), Some(task.clone()), None);

    for category in ALL_HIGH_RISK_CATEGORY_DTOS {
        let before = high_risk_approval::handle_get_high_risk_approval_status(
            &runtime,
            &task.id().to_string(),
            task.version(),
            category,
        )
        .unwrap_or_else(|error| panic!("status lookup for {category:?}: {error:?}"));
        assert!(!before.approved, "{category:?} must start unapproved");

        let approval = high_risk_approval::handle_approve_high_risk_operation(
            &runtime,
            &task.id().to_string(),
            task.version(),
            category,
        )
        .unwrap_or_else(|error| panic!("approve for {category:?}: {error:?}"));
        assert_eq!(approval.risk_category, category);

        let after = high_risk_approval::handle_get_high_risk_approval_status(
            &runtime,
            &task.id().to_string(),
            task.version(),
            category,
        )
        .unwrap_or_else(|error| panic!("status lookup after approve for {category:?}: {error:?}"));
        assert!(
            after.approved,
            "{category:?} must be approved after the call"
        );
    }
}

#[test]
fn cancel_claude_review_reports_whether_a_matching_run_was_found() {
    let calls = Arc::new(CallCounts::default());
    let runtime = ready_runtime(calls);
    let task_id = TaskId::new();

    let none_found = review::handle_cancel_claude_review(&runtime, &task_id.to_string())
        .expect("cancel never fails, even with nothing to cancel");
    assert!(!none_found.requested);

    let ready = runtime.ready_snapshot().expect("ready runtime");
    let _signal = ready
        .review_runs
        .register(task_id)
        .expect("first registration for this task id");
    let found = review::handle_cancel_claude_review(&runtime, &task_id.to_string())
        .expect("cancel a registered run");
    assert!(found.requested);
}

fn review_result_record(
    task_id: TaskId,
    review_text: &str,
) -> chatoms_ports::repository::TaskReviewResultRecord {
    chatoms_ports::repository::TaskReviewResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: chatoms_domain::WorkKind::Review,
        outcome: chatoms_ports::repository::ReviewResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(2),
        started_at_ms: 10,
        completed_at_ms: 20,
        review_text: Some(review_text.to_owned()),
    }
}

#[test]
fn get_review_result_returns_the_stored_result_only_in_awaiting_user_diff_approval() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingUserDiffApproval);
    let record = review_result_record(task.id(), "The change matches the requirements.");
    let runtime = ready_runtime_with_task_and_review_result(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(record),
    );

    let result = review::handle_get_review_result(&runtime, &task.id().to_string())
        .expect("review result lookup succeeds")
        .expect("a result was recorded for this task");
    assert_eq!(
        result.review_text.as_deref(),
        Some("The change matches the requirements.")
    );
    assert_eq!(result.outcome, crate::dto::ReviewOutcomeDto::Completed);
}

#[test]
fn get_review_result_is_hidden_outside_awaiting_user_diff_approval() {
    let task = task_in_state(chatoms_domain::TaskState::Reviewing);
    let record = review_result_record(task.id(), "Should never surface.");
    let runtime = ready_runtime_with_task_and_review_result(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(record),
    );

    let result = review::handle_get_review_result(&runtime, &task.id().to_string())
        .expect("review result lookup succeeds");
    assert!(
        result.is_none(),
        "a task outside AwaitingUserDiffApproval must never expose its review result"
    );
}

#[test]
fn get_review_result_reports_no_result_when_none_is_recorded_yet() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingUserDiffApproval);
    let runtime = ready_runtime_with_task_and_review_result(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
    );

    let result = review::handle_get_review_result(&runtime, &task.id().to_string())
        .expect("review result lookup succeeds");
    assert!(result.is_none());
}

#[test]
fn get_review_result_for_a_missing_task_is_a_safe_not_found_error() {
    let runtime =
        ready_runtime_with_task_and_review_result(Arc::new(CallCounts::default()), None, None);

    let error = review::handle_get_review_result(&runtime, &TaskId::new().to_string())
        .expect_err("missing task");
    assert_eq!(error.code, "APP_NOT_FOUND");
}

fn context_package_preparation(
    task_id: TaskId,
    work_kind: chatoms_domain::WorkKind,
    approved_task_version: u64,
) -> ContextPackagePreparation {
    ContextPackagePreparation {
        consent: chatoms_ports::repository::ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind,
            approved_task_version,
            data_scope: chatoms_domain::ContextDataScope::ContextPackageV1,
            consented_at_ms: 200,
        },
        manifest: chatoms_ports::repository::ContextPackageManifestRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind,
            approved_task_version,
            data_scope: chatoms_domain::ContextDataScope::ContextPackageV1,
            created_at_ms: 210,
        },
    }
}

#[test]
fn prepare_planning_context_package_succeeds_and_never_touches_task_state_or_version() {
    let task = task_in_state(chatoms_domain::TaskState::WorktreeReady);
    let isolation = worktree_ready_isolation(
        task.id(),
        task.project_id(),
        std::path::Path::new("C:/managed/task"),
        task.version(),
    );
    let preparation = context_package_preparation(
        task.id(),
        chatoms_domain::WorkKind::Planning,
        task.version(),
    );
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(isolation),
        Some(Ok(preparation)),
    );

    let dto = context_package::handle_prepare_planning_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect("preparation succeeds");

    assert_eq!(dto.work_kind, crate::dto::WorkKindDto::Planning);
    assert_eq!(
        dto.data_scope,
        crate::dto::ContextPackageDataScopeDto::ContextPackageV1
    );
    assert_eq!(dto.consented_at_ms, 200);
    assert_eq!(dto.manifest_created_at_ms, 210);

    // No background execution: this fake never records a state-changing
    // call, so if the handler had tried to save a transition or terminate
    // the task, the fake's `save_transition`/`terminate_task` overrides
    // (both of which unconditionally return `operation_failed()`) would
    // have made the call above fail instead of succeeding.
}

#[test]
fn prepare_planning_context_package_propagates_a_wrong_state_precondition_failure() {
    let task = task_in_state(chatoms_domain::TaskState::Planning);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        None,
    );

    let error = context_package::handle_prepare_planning_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect_err("Planning must not be accepted as WorktreeReady");
    assert_eq!(error.code, "APP_INVALID_STATE");
}

#[test]
fn prepare_planning_context_package_propagates_a_stale_version_precondition_failure() {
    let task = task_in_state(chatoms_domain::TaskState::WorktreeReady);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        None,
    );

    let error = context_package::handle_prepare_planning_context_package(
        &runtime,
        &task.id().to_string(),
        task.version() + 41,
    )
    .expect_err("a stale expected_version must be rejected");
    assert_eq!(error.code, "APP_VERSION_CONFLICT");
}

#[test]
fn prepare_planning_context_package_propagates_a_repository_failure_without_converting_it_to_success()
 {
    let task = task_in_state(chatoms_domain::TaskState::WorktreeReady);
    let isolation = worktree_ready_isolation(
        task.id(),
        task.project_id(),
        std::path::Path::new("C:/managed/task"),
        task.version(),
    );
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        Some(isolation),
        Some(Err(RepositoryErrorCode::InvalidPersistenceState)),
    );

    let error = context_package::handle_prepare_planning_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect_err("a repository failure must propagate as an error, never a success");
    assert_eq!(error.code, "APP_INTERNAL");
}

#[test]
fn prepare_implementation_context_package_succeeds_and_never_touches_task_state_or_version() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let preparation = context_package_preparation(
        task.id(),
        chatoms_domain::WorkKind::Implementation,
        task.version(),
    );
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        Some(Ok(preparation)),
    );

    let dto = context_package::handle_prepare_implementation_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect("preparation succeeds");

    assert_eq!(dto.work_kind, crate::dto::WorkKindDto::Implementation);
    assert_eq!(
        dto.data_scope,
        crate::dto::ContextPackageDataScopeDto::ContextPackageV1
    );
}

#[test]
fn prepare_implementation_context_package_propagates_a_wrong_state_precondition_failure() {
    let task = task_in_state(chatoms_domain::TaskState::WorktreeReady);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        None,
    );

    let error = context_package::handle_prepare_implementation_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect_err("WorktreeReady must not be accepted as AwaitingDesignApproval");
    assert_eq!(error.code, "APP_INVALID_STATE");
}

#[test]
fn prepare_implementation_context_package_propagates_a_repository_failure() {
    let task = task_in_state(chatoms_domain::TaskState::AwaitingDesignApproval);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        Some(Err(RepositoryErrorCode::InvalidPersistenceState)),
    );

    let error = context_package::handle_prepare_implementation_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect_err("a repository failure must propagate as an error, never a success");
    assert_eq!(error.code, "APP_INTERNAL");
}

#[test]
fn prepare_review_context_package_succeeds_and_never_touches_task_state_or_version() {
    let task = task_in_state(chatoms_domain::TaskState::Reviewing);
    let preparation =
        context_package_preparation(task.id(), chatoms_domain::WorkKind::Review, task.version());
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        Some(Ok(preparation)),
    );

    let dto = context_package::handle_prepare_review_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect("preparation succeeds");

    assert_eq!(dto.work_kind, crate::dto::WorkKindDto::Review);
    assert_eq!(
        dto.data_scope,
        crate::dto::ContextPackageDataScopeDto::ContextPackageV1
    );
}

#[test]
fn prepare_review_context_package_propagates_a_wrong_state_precondition_failure() {
    let task = task_in_state(chatoms_domain::TaskState::Testing);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        None,
    );

    let error = context_package::handle_prepare_review_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect_err("Testing must not be accepted as Reviewing");
    assert_eq!(error.code, "APP_INVALID_STATE");
}

#[test]
fn prepare_review_context_package_propagates_a_repository_failure() {
    let task = task_in_state(chatoms_domain::TaskState::Reviewing);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        Some(task.clone()),
        None,
        Some(Err(RepositoryErrorCode::InvalidPersistenceState)),
    );

    let error = context_package::handle_prepare_review_context_package(
        &runtime,
        &task.id().to_string(),
        task.version(),
    )
    .expect_err("a repository failure must propagate as an error, never a success");
    assert_eq!(error.code, "APP_INTERNAL");
}

#[test]
fn get_context_package_planning_readiness_is_true_when_the_pair_is_prepared() {
    let task_id = TaskId::new();
    let preparation = context_package_preparation(task_id, chatoms_domain::WorkKind::Planning, 1);
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        None,
        None,
        Some(Ok(preparation)),
    );

    let dto = context_package::handle_get_context_package_planning_readiness(
        &runtime,
        &task_id.to_string(),
        1,
    )
    .expect("readiness lookup succeeds");

    assert!(dto.ready);
}

#[test]
fn get_context_package_planning_readiness_is_false_when_nothing_is_prepared() {
    let task_id = TaskId::new();
    let runtime = ready_runtime_with_task_isolation_and_context_package_outcome(
        Arc::new(CallCounts::default()),
        None,
        None,
        None,
    );

    let dto = context_package::handle_get_context_package_planning_readiness(
        &runtime,
        &task_id.to_string(),
        1,
    )
    .expect("readiness lookup succeeds");

    assert!(!dto.ready);
}

#[test]
fn get_user_diff_for_review_rejects_an_unparseable_task_id_before_touching_git_or_repository() {
    let runtime = ready_runtime(Arc::new(CallCounts::default()));
    let error = user_diff_review::handle_get_user_diff_for_review(&runtime, "not-a-task-id", 1)
        .expect_err("malformed task id must be rejected");
    assert_eq!(error.code, "APP_INVALID_INPUT");
}

#[test]
fn approve_user_diff_rejects_an_unparseable_task_id_before_touching_git_or_repository() {
    let runtime = ready_runtime(Arc::new(CallCounts::default()));
    let error =
        user_diff_review::handle_approve_user_diff(&runtime, "not-a-task-id", 1, &"a".repeat(64))
            .expect_err("malformed task id must be rejected");
    assert_eq!(error.code, "APP_INVALID_INPUT");
}

#[test]
fn approve_user_diff_rejects_a_malformed_hash_before_touching_git_or_repository() {
    let runtime = ready_runtime(Arc::new(CallCounts::default()));
    for malformed in ["", "not-hex", &"a".repeat(63), &"A".repeat(64)] {
        let error = user_diff_review::handle_approve_user_diff(
            &runtime,
            &TaskId::new().to_string(),
            1,
            malformed,
        )
        .expect_err("malformed hash must be rejected");
        assert_eq!(error.code, "APP_INVALID_INPUT");
    }
}
