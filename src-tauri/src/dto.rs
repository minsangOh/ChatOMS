use chatoms_application::{
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
    git_isolation::{IsolationBlocker, TaskIsolationView},
    projects::{ProjectCandidateView, ProjectStatusView, ProjectView},
    system::{CapabilityStatus, HealthStatus, SystemStatus},
    tasks::{ActiveTaskView, TaskTransitionView, TaskView},
};
use chatoms_domain::TaskState;
use chatoms_ports::{
    git::{RepositoryKind, RepositoryStatus},
    repository::GitIsolationStatus,
};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDto {
    pub version: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthStateDto {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDto {
    pub status: HealthStateDto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StorageStatusDto {
    Ready,
    Unavailable,
    Insecure,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseStatusDto {
    NotChecked,
    Ready,
    Upgraded,
    MigrationRequired,
    Unavailable,
    Incompatible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LoggingStatusDto {
    NotChecked,
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActiveTaskStatusKindDto {
    NotChecked,
    None,
    Active,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTaskStatusDto {
    pub status: ActiveTaskStatusKindDto,
    pub task_id: Option<String>,
    pub acquired_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapStatusDto {
    pub storage_status: StorageStatusDto,
    pub database_status: DatabaseStatusDto,
    pub logging_status: LoggingStatusDto,
    pub active_task_status: ActiveTaskStatusDto,
    pub application_version: &'static str,
    pub ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationDiagnosticDto {
    pub project_id: String,
    pub display_path: String,
    pub reason_code: String,
}

impl From<chatoms_infrastructure::bootstrap::LegacyMigrationDiagnostic>
    for LegacyMigrationDiagnosticDto
{
    fn from(value: chatoms_infrastructure::bootstrap::LegacyMigrationDiagnostic) -> Self {
        Self {
            project_id: value.project_id,
            display_path: value.display_path,
            reason_code: value.reason_code.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityStatusDto {
    Supported,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDto {
    pub secure_storage: CapabilityStatusDto,
    pub native_permissions: CapabilityStatusDto,
    pub git_execution: CapabilityStatusDto,
    pub claude_execution: CapabilityStatusDto,
    pub codex_execution: CapabilityStatusDto,
    pub updater: CapabilityStatusDto,
    pub installer_management: CapabilityStatusDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatusDto {
    pub application_version: &'static str,
    pub health: HealthStateDto,
    pub storage_status: StorageStatusDto,
    pub database_status: DatabaseStatusDto,
    pub logging_status: LoggingStatusDto,
    pub active_task_status: ActiveTaskStatusDto,
    pub capabilities: CapabilityDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: String,
    pub name: String,
    pub display_path: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryKindDto {
    Git,
    NonGit,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryStatusDto {
    pub clean: bool,
    pub detached_head: bool,
    pub current_branch: Option<String>,
    pub head_commit: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCandidateDto {
    pub suggested_name: String,
    pub display_path: String,
    pub confirmation_token: String,
    pub repository_kind: RepositoryKindDto,
    pub repository_status: Option<RepositoryStatusDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatusDto {
    pub project_id: String,
    pub repository_kind: RepositoryKindDto,
    pub repository_status: Option<RepositoryStatusDto>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitIsolationStatusDto {
    AwaitingGitInitApproval,
    Ready,
    GitInitInProgress,
    WorktreeCreating,
    WorktreeReady,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IsolationBlockerDto {
    DirtyRepository,
    DetachedHead,
    UnbornRepository,
    MissingCurrentBranch,
    GitAuthorMissing,
    GitOperationFailed,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskIsolationDto {
    pub task_id: String,
    pub project_id: String,
    pub task_state: TaskStateDto,
    pub task_version: u64,
    pub isolation_status: GitIsolationStatusDto,
    pub branch_identity: String,
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
    pub blocker: Option<IsolationBlockerDto>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStateDto {
    Created,
    ProjectValidated,
    AwaitingGitInitApproval,
    GitInitialized,
    WorktreeCreating,
    WorktreeReady,
    PlanningWithClaude,
    AwaitingDesignApproval,
    ImplementingWithCodex,
    Testing,
    AutoFixing,
    ReviewingWithClaude,
    ReviewFixing,
    AwaitingUserDiffApproval,
    Merging,
    MergeConflict,
    PostMergeTesting,
    Completed,
    Paused,
    Failed,
    RecoveryRequired,
    UnknownExternalEffect,
    Cancelled,
    CleanupPending,
    Archived,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTaskDto {
    pub task_id: String,
    pub acquired_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDto {
    pub id: String,
    pub project_id: String,
    pub state: TaskStateDto,
    pub version: u64,
    pub branch_identity: String,
    pub resume_target_state: Option<TaskStateDto>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTransitionDto {
    pub sequence: u64,
    pub from_state: Option<TaskStateDto>,
    pub to_state: TaskStateDto,
    pub task_version: u64,
    pub occurred_at_ms: i64,
}

impl From<BootstrapStatus> for BootstrapStatusDto {
    fn from(value: BootstrapStatus) -> Self {
        Self {
            storage_status: value.storage_status.into(),
            database_status: value.database_status.into(),
            logging_status: value.logging_status.into(),
            active_task_status: value.active_task_status.into(),
            application_version: value.application_version,
            ready: value.ready,
        }
    }
}

impl From<SystemStatus> for SystemStatusDto {
    fn from(value: SystemStatus) -> Self {
        Self {
            application_version: value.application_version,
            health: value.health.into(),
            storage_status: value.storage_status.into(),
            database_status: value.database_status.into(),
            logging_status: value.logging_status.into(),
            active_task_status: value.active_task_status.into(),
            capabilities: CapabilityDto {
                secure_storage: value.capabilities.secure_storage.into(),
                native_permissions: value.capabilities.native_permissions.into(),
                git_execution: CapabilityStatusDto::Unavailable,
                claude_execution: CapabilityStatusDto::Unavailable,
                codex_execution: CapabilityStatusDto::Unavailable,
                updater: CapabilityStatusDto::Unavailable,
                installer_management: CapabilityStatusDto::Unavailable,
            },
        }
    }
}

impl From<ProjectView> for ProjectDto {
    fn from(value: ProjectView) -> Self {
        Self {
            id: value.id().to_string(),
            name: value.name().to_owned(),
            display_path: value.display_path().to_owned(),
            created_at_ms: value.created_at_ms(),
            updated_at_ms: value.updated_at_ms(),
        }
    }
}

impl From<RepositoryKind> for RepositoryKindDto {
    fn from(value: RepositoryKind) -> Self {
        match value {
            RepositoryKind::Git => Self::Git,
            RepositoryKind::NonGit => Self::NonGit,
        }
    }
}
impl From<RepositoryStatus> for RepositoryStatusDto {
    fn from(value: RepositoryStatus) -> Self {
        Self {
            clean: value.clean,
            detached_head: value.detached_head,
            current_branch: value.current_branch,
            head_commit: value.head_commit,
        }
    }
}
impl From<ProjectCandidateView> for ProjectCandidateDto {
    fn from(value: ProjectCandidateView) -> Self {
        Self {
            suggested_name: value.suggested_name,
            display_path: value.display_path,
            confirmation_token: value.confirmation_token,
            repository_kind: value.repository_kind.into(),
            repository_status: value.repository_status.map(Into::into),
        }
    }
}
impl From<ProjectStatusView> for ProjectStatusDto {
    fn from(value: ProjectStatusView) -> Self {
        Self {
            project_id: value.project_id.to_string(),
            repository_kind: value.repository_kind.into(),
            repository_status: value.repository_status.map(Into::into),
        }
    }
}
impl From<GitIsolationStatus> for GitIsolationStatusDto {
    fn from(value: GitIsolationStatus) -> Self {
        match value {
            GitIsolationStatus::AwaitingGitInitApproval => Self::AwaitingGitInitApproval,
            GitIsolationStatus::Ready => Self::Ready,
            GitIsolationStatus::GitInitInProgress => Self::GitInitInProgress,
            GitIsolationStatus::WorktreeCreating => Self::WorktreeCreating,
            GitIsolationStatus::WorktreeReady => Self::WorktreeReady,
            GitIsolationStatus::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}
impl From<IsolationBlocker> for IsolationBlockerDto {
    fn from(value: IsolationBlocker) -> Self {
        match value {
            IsolationBlocker::DirtyRepository => Self::DirtyRepository,
            IsolationBlocker::DetachedHead => Self::DetachedHead,
            IsolationBlocker::UnbornRepository => Self::UnbornRepository,
            IsolationBlocker::MissingCurrentBranch => Self::MissingCurrentBranch,
            IsolationBlocker::GitAuthorMissing => Self::GitAuthorMissing,
            IsolationBlocker::GitOperationFailed => Self::GitOperationFailed,
            IsolationBlocker::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}
impl From<TaskIsolationView> for TaskIsolationDto {
    fn from(value: TaskIsolationView) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            project_id: value.project_id.to_string(),
            task_state: value.task_state.into(),
            task_version: value.task_version,
            isolation_status: value.isolation_status.into(),
            branch_identity: value.branch_identity,
            base_branch: value.base_branch,
            base_commit: value.base_commit,
            blocker: value.blocker.map(Into::into),
        }
    }
}

impl From<ActiveTaskView> for ActiveTaskDto {
    fn from(value: ActiveTaskView) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            acquired_at_ms: value.acquired_at_ms,
        }
    }
}

impl From<TaskView> for TaskDto {
    fn from(value: TaskView) -> Self {
        Self {
            id: value.id.to_string(),
            project_id: value.project_id.to_string(),
            state: value.state.into(),
            version: value.version,
            branch_identity: value.branch_identity.to_string(),
            resume_target_state: value.resume_target_state.map(TaskStateDto::from),
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
            terminal_at_ms: value.terminal_at_ms,
        }
    }
}

impl From<TaskTransitionView> for TaskTransitionDto {
    fn from(value: TaskTransitionView) -> Self {
        Self {
            sequence: value.sequence,
            from_state: value.from_state.map(TaskStateDto::from),
            to_state: value.to_state.into(),
            task_version: value.task_version,
            occurred_at_ms: value.occurred_at_ms,
        }
    }
}

impl From<HealthStatus> for HealthStateDto {
    fn from(value: HealthStatus) -> Self {
        match value {
            HealthStatus::Healthy => Self::Healthy,
            HealthStatus::Degraded => Self::Degraded,
            HealthStatus::Unavailable => Self::Unavailable,
        }
    }
}

impl From<StorageStatus> for StorageStatusDto {
    fn from(value: StorageStatus) -> Self {
        match value {
            StorageStatus::Ready => Self::Ready,
            StorageStatus::Unavailable => Self::Unavailable,
            StorageStatus::Insecure => Self::Insecure,
            StorageStatus::Unsupported => Self::Unsupported,
        }
    }
}

impl From<DatabaseStatus> for DatabaseStatusDto {
    fn from(value: DatabaseStatus) -> Self {
        match value {
            DatabaseStatus::NotChecked => Self::NotChecked,
            DatabaseStatus::Ready => Self::Ready,
            DatabaseStatus::Upgraded => Self::Upgraded,
            DatabaseStatus::MigrationRequired => Self::MigrationRequired,
            DatabaseStatus::Unavailable => Self::Unavailable,
            DatabaseStatus::Incompatible => Self::Incompatible,
        }
    }
}

impl From<LoggingStatus> for LoggingStatusDto {
    fn from(value: LoggingStatus) -> Self {
        match value {
            LoggingStatus::NotChecked => Self::NotChecked,
            LoggingStatus::Ready => Self::Ready,
            LoggingStatus::Unavailable => Self::Unavailable,
        }
    }
}

impl From<ActiveTaskStatus> for ActiveTaskStatusDto {
    fn from(value: ActiveTaskStatus) -> Self {
        match value {
            ActiveTaskStatus::NotChecked => Self {
                status: ActiveTaskStatusKindDto::NotChecked,
                task_id: None,
                acquired_at_ms: None,
            },
            ActiveTaskStatus::None => Self {
                status: ActiveTaskStatusKindDto::None,
                task_id: None,
                acquired_at_ms: None,
            },
            ActiveTaskStatus::Active {
                task_id,
                acquired_at_ms,
            } => Self {
                status: ActiveTaskStatusKindDto::Active,
                task_id: Some(task_id.to_string()),
                acquired_at_ms: Some(acquired_at_ms),
            },
        }
    }
}

impl From<CapabilityStatus> for CapabilityStatusDto {
    fn from(value: CapabilityStatus) -> Self {
        match value {
            CapabilityStatus::Supported => Self::Supported,
            CapabilityStatus::Unsupported => Self::Unsupported,
        }
    }
}

impl From<TaskState> for TaskStateDto {
    fn from(value: TaskState) -> Self {
        match value {
            TaskState::Created => Self::Created,
            TaskState::ProjectValidated => Self::ProjectValidated,
            TaskState::AwaitingGitInitApproval => Self::AwaitingGitInitApproval,
            TaskState::GitInitialized => Self::GitInitialized,
            TaskState::WorktreeCreating => Self::WorktreeCreating,
            TaskState::WorktreeReady => Self::WorktreeReady,
            TaskState::PlanningWithClaude => Self::PlanningWithClaude,
            TaskState::AwaitingDesignApproval => Self::AwaitingDesignApproval,
            TaskState::ImplementingWithCodex => Self::ImplementingWithCodex,
            TaskState::Testing => Self::Testing,
            TaskState::AutoFixing => Self::AutoFixing,
            TaskState::ReviewingWithClaude => Self::ReviewingWithClaude,
            TaskState::ReviewFixing => Self::ReviewFixing,
            TaskState::AwaitingUserDiffApproval => Self::AwaitingUserDiffApproval,
            TaskState::Merging => Self::Merging,
            TaskState::MergeConflict => Self::MergeConflict,
            TaskState::PostMergeTesting => Self::PostMergeTesting,
            TaskState::Completed => Self::Completed,
            TaskState::Paused => Self::Paused,
            TaskState::Failed => Self::Failed,
            TaskState::RecoveryRequired => Self::RecoveryRequired,
            TaskState::UnknownExternalEffect => Self::UnknownExternalEffect,
            TaskState::Cancelled => Self::Cancelled,
            TaskState::CleanupPending => Self::CleanupPending,
            TaskState::Archived => Self::Archived,
        }
    }
}

#[cfg(test)]
mod tests {
    use chatoms_domain::TaskId;
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    fn json(response: impl IpcResponse) -> String {
        let InvokeResponseBody::Json(json) = response.body().expect("JSON serialization") else {
            panic!("expected JSON response");
        };
        json
    }

    #[test]
    fn dto_serialization_is_camel_case_stable_and_path_free() {
        let project = ProjectDto {
            id: TaskId::new().to_string(),
            name: "Foundation".to_owned(),
            display_path: "%USERPROFILE%\\Foundation".to_owned(),
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let serialized = json(project);
        assert!(serialized.contains("\"createdAtMs\":1"));
        assert!(serialized.contains("\"updatedAtMs\":2"));
        assert!(!serialized.contains("rootPath"));
        assert!(!serialized.contains("C:\\\\"));

        let health = json(HealthDto {
            status: HealthStateDto::Degraded,
        });
        assert_eq!(health, "{\"status\":\"degraded\"}");
    }

    #[test]
    fn uuid_timestamp_and_null_are_serialized_without_type_leakage() {
        let id = TaskId::new().to_string();
        assert_eq!(id, id.to_lowercase());
        let task = TaskDto {
            id: id.clone(),
            project_id: TaskId::new().to_string(),
            state: TaskStateDto::Created,
            version: 0,
            branch_identity: format!("ai-task/{id}"),
            resume_target_state: None,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
            terminal_at_ms: None,
        };
        let serialized = json(task);
        assert!(serialized.contains("\"state\":\"created\""));
        assert!(serialized.contains("\"resumeTargetState\":null"));
        assert!(serialized.contains("\"terminalAtMs\":null"));
        assert!(serialized.contains("1700000000000"));
    }
}
