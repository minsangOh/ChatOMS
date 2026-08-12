use std::str::FromStr;

use chatoms_domain::{
    ActorKind, GitOperationId, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId,
    TaskSnapshot, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot,
};
use chatoms_ports::git::RepositoryKind;
use chatoms_ports::provider::ProviderKind;
use chatoms_ports::repository::{
    ActiveLease, AppProfileRecord, FoundationRepository, GitInitApproval, GitIsolationStatus,
    GitOperationAttempt, GitOperationAttemptStatus, GitOperationKind, GitOperationReceipt,
    GitOperationReceiptKind, ProjectFilesystemIdentityRecord, ProjectRecord, ProjectSummary,
    ProviderBindingRecord, RepositoryError, RepositoryErrorCode, TaskGitIsolation,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::DatabaseConnection;

pub struct SqliteFoundationRepository<'connection> {
    database: &'connection mut DatabaseConnection,
}

impl<'connection> SqliteFoundationRepository<'connection> {
    pub fn new(database: &'connection mut DatabaseConnection) -> Self {
        Self { database }
    }
}

impl FoundationRepository for SqliteFoundationRepository<'_> {
    fn create_project(&mut self, project: &ProjectRecord) -> Result<(), RepositoryError> {
        validate_project(project)?;
        self.database
            .raw_mut()
            .execute(
                "INSERT INTO projects (
                    id, name, root_path, canonical_path_key, display_path,
                    created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    project.id.to_string(),
                    project.name,
                    project.root_path,
                    project.canonical_path_key,
                    project.display_path,
                    project.created_at_ms,
                    project.updated_at_ms
                ],
            )
            .map_err(|source| {
                RepositoryError::with_source(RepositoryErrorCode::DuplicateProject, source)
            })?;
        Ok(())
    }

    fn create_project_with_identity(
        &mut self,
        project: &ProjectRecord,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        validate_project(project)?;
        validate_project_identity(identity)?;
        if identity.project_id != project.id {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        transaction
            .execute(
                "INSERT INTO projects (
                    id, name, root_path, canonical_path_key, display_path,
                    created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    project.id.to_string(),
                    project.name,
                    project.root_path,
                    project.canonical_path_key,
                    project.display_path,
                    project.created_at_ms,
                    project.updated_at_ms
                ],
            )
            .map_err(|source| {
                RepositoryError::with_source(RepositoryErrorCode::DuplicateProject, source)
            })?;
        insert_project_identity(&transaction, identity)?;
        transaction.commit().map_err(operation_failed)
    }

    fn get_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        load_project_identity(self.database.raw_mut(), project_id)
    }

    fn update_project_identity(
        &mut self,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        validate_project_identity(identity)?;
        update_project_identity_row(self.database.raw_mut(), identity)
    }

    fn get_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        load_project(self.database.raw_mut(), project_id)
    }

    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        validate_new_task(task, initial_transition, lease_acquired_at_ms)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;

        if !project_exists(&transaction, task.project_id())? {
            return Err(repository_error(RepositoryErrorCode::ProjectNotFound));
        }
        if task_exists(&transaction, task.id())? {
            return Err(repository_error(RepositoryErrorCode::DuplicateTask));
        }
        if query_active_lease(&transaction)?.is_some() {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }

        insert_task(&transaction, task).map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::DuplicateTask, source)
        })?;
        insert_transition(&transaction, initial_transition).map_err(operation_failed)?;
        transaction
            .execute(
                "INSERT INTO active_task_leases (singleton_key, task_id, acquired_at_ms)
                 VALUES (1, ?1, ?2)",
                params![task.id().to_string(), lease_acquired_at_ms],
            )
            .map_err(|source| {
                RepositoryError::with_source(RepositoryErrorCode::ActiveLeaseConflict, source)
            })?;
        transaction.commit().map_err(operation_failed)
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        load_task(self.database.raw_mut(), task_id)
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        if task.state().is_terminal() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        task.validate_invariants()
            .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
        validate_nonnegative_task(task)?;

        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;

        let lease = query_active_lease(&transaction)?;
        if task.state().requires_active_lease() {
            if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
                return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
            }
        } else if lease
            .as_ref()
            .is_some_and(|active| active.task_id == task.id())
        {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }

        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_recovery_target(
        &mut self,
        expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        task.validate_invariants()
            .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
        validate_nonnegative_task(task)?;
        if task.state() != TaskState::RecoveryRequired
            || task.resume_target_state().is_none()
            || task.version() != expected_version
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }

        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::RecoveryRequired
            || current.resume_target_state().is_some()
            || current.project_id() != task.project_id()
            || current.task_branch_identity() != task.task_branch_identity()
            || current.created_at_ms() != task.created_at_ms()
            || current.updated_at_ms() != task.updated_at_ms()
            || current.terminal_at_ms() != task.terminal_at_ms()
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }

        let changed = transaction
            .execute(
                "UPDATE tasks
                 SET resume_target_state = ?1
                 WHERE id = ?2
                   AND version = ?3
                   AND state = 'RecoveryRequired'
                   AND resume_target_state IS NULL",
                params![
                    task.resume_target_state().map(state_text),
                    task.id().to_string(),
                    to_sql_integer(expected_version)
                        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?
                ],
            )
            .map_err(operation_failed)?;
        if changed != 1 {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        transaction.commit().map_err(operation_failed)
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        if !task.state().is_terminal() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        task.validate_invariants()
            .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
        validate_nonnegative_task(task)?;

        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if query_active_lease(&transaction)?
            .as_ref()
            .map(|active| active.task_id)
            != Some(task.id())
        {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;

        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        let deleted = transaction
            .execute(
                "DELETE FROM active_task_leases WHERE task_id = ?1",
                [task.id().to_string()],
            )
            .map_err(|source| {
                RepositoryError::with_source(RepositoryErrorCode::ActiveLeaseConflict, source)
            })?;
        if deleted != 1 {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        transaction.commit().map_err(operation_failed)
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        load_and_validate_transitions(self.database.raw_mut(), task_id)
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        load_projects(self.database.raw_mut())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        query_active_lease(self.database.raw_mut())
    }

    fn create_isolation_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        classified_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        validate_new_isolation_task(
            task,
            initial_transition,
            classified_transition,
            lease_acquired_at_ms,
            isolation,
        )?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        if !project_exists(&transaction, task.project_id())? {
            return Err(repository_error(RepositoryErrorCode::ProjectNotFound));
        }
        if task_exists(&transaction, task.id())? {
            return Err(repository_error(RepositoryErrorCode::DuplicateTask));
        }
        if query_active_lease(&transaction)?.is_some() {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        insert_task(&transaction, task).map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::DuplicateTask, source)
        })?;
        insert_transition(&transaction, initial_transition).map_err(operation_failed)?;
        insert_transition(&transaction, classified_transition).map_err(operation_failed)?;
        transaction
            .execute(
                "INSERT INTO active_task_leases (singleton_key, task_id, acquired_at_ms)
                 VALUES (1, ?1, ?2)",
                params![task.id().to_string(), lease_acquired_at_ms],
            )
            .map_err(|source| {
                RepositoryError::with_source(RepositoryErrorCode::ActiveLeaseConflict, source)
            })?;
        insert_isolation(&transaction, isolation)?;
        transaction.commit().map_err(operation_failed)
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        load_isolation(self.database.raw_mut(), task_id)
    }

    fn begin_git_initialization(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
        approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        if isolation.status != GitIsolationStatus::GitInitInProgress
            || isolation.operation_id != Some(approval.operation_id)
            || isolation.task_id != approval.task_id
            || isolation.project_id != approval.project_id
            || approval.approved_task_version != expected_version
            || isolation.expected_task_version != expected_version
            || approval.approved_at_ms < 0
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        validate_isolation_expected_version(&transaction, isolation.task_id, expected_version)?;
        update_isolation(&transaction, isolation)?;
        insert_git_operation_attempt(&transaction, isolation, GitOperationKind::GitInitialize)?;
        transaction
            .execute(
                "INSERT INTO git_init_approvals (
                    operation_id, task_id, project_id, approved_task_version, approved_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    approval.operation_id.to_string(),
                    approval.task_id.to_string(),
                    approval.project_id.to_string(),
                    to_sql_integer(approval.approved_task_version)
                        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                    approval.approved_at_ms
                ],
            )
            .map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_isolation_intent(
        &mut self,
        expected_version: u64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        validate_isolation_expected_version(&transaction, isolation.task_id, expected_version)?;
        update_isolation(&transaction, isolation)?;
        transaction.commit().map_err(operation_failed)
    }

    fn append_git_operation_receipt(
        &mut self,
        operation_id: GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        if recorded_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        insert_operation_receipt(&transaction, operation_id, kind, evidence, recorded_at_ms)?;
        transaction.commit().map_err(operation_failed)
    }

    fn list_git_operation_receipts(
        &mut self,
        operation_id: GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        load_operation_receipts(self.database.raw_mut(), operation_id)
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        load_incomplete_operations(self.database.raw_mut())
    }

    fn save_isolation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        if task.state().is_terminal() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        persist_isolation_transition(
            &transaction,
            expected_version,
            task,
            transition,
            isolation,
            false,
        )?;
        if isolation.status == GitIsolationStatus::WorktreeCreating {
            insert_git_operation_attempt(
                &transaction,
                isolation,
                GitOperationKind::WorktreeCreate,
            )?;
        }
        if isolation.status == GitIsolationStatus::RecoveryRequired {
            complete_operation_attempt(
                &transaction,
                isolation,
                "RecoveryRequired",
                GitOperationReceiptKind::RecoveryRequired,
            )?;
        }
        transaction.commit().map_err(operation_failed)
    }

    fn save_git_initialization_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        if task.state() != TaskState::GitInitialized
            || isolation.status != GitIsolationStatus::Ready
            || identity.project_id != task.project_id()
            || identity.repository_kind != RepositoryKind::Git
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_project_identity(identity)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        persist_isolation_transition(
            &transaction,
            expected_version,
            task,
            transition,
            isolation,
            false,
        )?;
        update_project_identity_row(&transaction, identity)?;
        complete_operation_attempt(
            &transaction,
            isolation,
            "Completed",
            GitOperationReceiptKind::CompletionRecorded,
        )?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_worktree_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        if task.state() != TaskState::WorktreeReady
            || isolation.status != GitIsolationStatus::WorktreeReady
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        persist_isolation_transition(
            &transaction,
            expected_version,
            task,
            transition,
            isolation,
            false,
        )?;
        complete_operation_attempt(
            &transaction,
            isolation,
            "Completed",
            GitOperationReceiptKind::CompletionRecorded,
        )?;
        transaction.commit().map_err(operation_failed)
    }

    fn terminate_isolation_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        if !task.state().is_terminal() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        persist_isolation_transition(
            &transaction,
            expected_version,
            task,
            transition,
            isolation,
            true,
        )?;
        transaction.commit().map_err(operation_failed)
    }

    fn ensure_default_profile_and_claude_binding(
        &mut self,
        profile: &AppProfileRecord,
        binding: &ProviderBindingRecord,
    ) -> Result<ProviderBindingRecord, RepositoryError> {
        validate_profile_record(profile)?;
        validate_binding_record(binding)?;
        if binding.app_profile_id != profile.id {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        if binding.provider_kind != ProviderKind::Claude {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        transaction
            .execute(
                "INSERT INTO app_profiles (id, name, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (name) DO NOTHING",
                params![
                    profile.id,
                    profile.name,
                    profile.created_at_ms,
                    profile.updated_at_ms
                ],
            )
            .map_err(operation_failed)?;
        let existing_profile_id: String = transaction
            .query_row(
                "SELECT id FROM app_profiles WHERE name = ?1",
                [&profile.name],
                |row| row.get(0),
            )
            .map_err(operation_failed)?;
        transaction
            .execute(
                "INSERT INTO provider_bindings (
                    id, app_profile_id, provider_kind, display_name,
                    executable_path, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (app_profile_id, provider_kind) DO NOTHING",
                params![
                    binding.id,
                    existing_profile_id,
                    provider_kind_text(binding.provider_kind),
                    binding.display_name,
                    binding.executable_path,
                    binding.created_at_ms,
                    binding.updated_at_ms
                ],
            )
            .map_err(operation_failed)?;
        let result = load_binding_by_profile_and_kind(
            &transaction,
            &existing_profile_id,
            ProviderKind::Claude,
        )?
        .ok_or_else(|| repository_error(RepositoryErrorCode::OperationFailed))?;
        transaction.commit().map_err(operation_failed)?;
        Ok(result)
    }

    fn get_claude_binding(
        &mut self,
        profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        let profile_id: Option<String> = self
            .database
            .raw_mut()
            .query_row(
                "SELECT id FROM app_profiles WHERE name = ?1",
                [profile_name],
                |row| row.get(0),
            )
            .optional()
            .map_err(operation_failed)?;
        match profile_id {
            Some(id) => {
                load_binding_by_profile_and_kind(self.database.raw_mut(), &id, ProviderKind::Claude)
            }
            None => Ok(None),
        }
    }

    fn update_claude_executable_path(
        &mut self,
        binding_id: &str,
        executable_path: Option<&str>,
        updated_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        if updated_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current_kind: Option<String> = transaction
            .query_row(
                "SELECT provider_kind FROM provider_bindings WHERE id = ?1",
                [binding_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(operation_failed)?;
        match current_kind {
            None => return Err(repository_error(RepositoryErrorCode::BindingNotFound)),
            Some(kind) if kind != "Claude" => {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
            _ => {}
        }
        let changed = transaction
            .execute(
                "UPDATE provider_bindings
                 SET executable_path = ?1, updated_at_ms = ?2
                 WHERE id = ?3 AND provider_kind = 'Claude'",
                params![executable_path, updated_at_ms, binding_id],
            )
            .map_err(operation_failed)?;
        if changed != 1 {
            return Err(repository_error(RepositoryErrorCode::OperationFailed));
        }
        transaction.commit().map_err(operation_failed)
    }
}

fn validate_profile_record(profile: &AppProfileRecord) -> Result<(), RepositoryError> {
    if profile.id.is_empty()
        || profile.name.trim().is_empty()
        || profile.created_at_ms < 0
        || profile.updated_at_ms < profile.created_at_ms
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn validate_binding_record(binding: &ProviderBindingRecord) -> Result<(), RepositoryError> {
    if binding.id.is_empty()
        || binding.app_profile_id.is_empty()
        || binding.display_name.trim().is_empty()
        || binding.created_at_ms < 0
        || binding.updated_at_ms < binding.created_at_ms
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if binding
        .executable_path
        .as_deref()
        .is_some_and(|path| path.is_empty())
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn load_binding_by_profile_and_kind(
    connection: &Connection,
    profile_id: &str,
    kind: ProviderKind,
) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, app_profile_id, provider_kind, display_name,
                    executable_path, created_at_ms, updated_at_ms
             FROM provider_bindings
             WHERE app_profile_id = ?1 AND provider_kind = ?2",
            params![profile_id, provider_kind_text(kind)],
            |row| {
                Ok(ProviderBindingRecord {
                    id: row.get(0)?,
                    app_profile_id: row.get(1)?,
                    provider_kind: kind,
                    display_name: row.get(3)?,
                    executable_path: row.get(4)?,
                    created_at_ms: row.get(5)?,
                    updated_at_ms: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(operation_failed)
}

const fn provider_kind_text(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
    }
}

fn validate_project(project: &ProjectRecord) -> Result<(), RepositoryError> {
    if project.name.trim().is_empty()
        || project.root_path.is_empty()
        || project.canonical_path_key.is_empty()
        || project.display_path.is_empty()
        || project.created_at_ms < 0
        || project.updated_at_ms < project.created_at_ms
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn validate_project_identity(
    identity: &ProjectFilesystemIdentityRecord,
) -> Result<(), RepositoryError> {
    let root_valid = is_lower_hex(&identity.root_volume_serial_hex, 16)
        && is_lower_hex(&identity.root_file_id_hex, 32);
    let common_valid = match identity.repository_kind {
        RepositoryKind::Git => {
            identity
                .git_common_volume_serial_hex
                .as_deref()
                .is_some_and(|value| is_lower_hex(value, 16))
                && identity
                    .git_common_file_id_hex
                    .as_deref()
                    .is_some_and(|value| is_lower_hex(value, 32))
        }
        RepositoryKind::NonGit => {
            identity.git_common_volume_serial_hex.is_none()
                && identity.git_common_file_id_hex.is_none()
        }
    };
    if !root_valid || !common_valid || identity.revision == 0 || identity.verified_at_ms < 0 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_project_identity(
    connection: &Connection,
    identity: &ProjectFilesystemIdentityRecord,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO project_filesystem_identities (
                project_id, identity_scheme, root_volume_serial_hex, root_file_id_hex,
                repository_kind, git_common_volume_serial_hex, git_common_file_id_hex,
                confirmed, revision, verified_at_ms
             ) VALUES (?1, 'WindowsFileIdV1', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                identity.project_id.to_string(),
                identity.root_volume_serial_hex,
                identity.root_file_id_hex,
                repository_kind_text(identity.repository_kind),
                identity.git_common_volume_serial_hex,
                identity.git_common_file_id_hex,
                i64::from(identity.confirmed),
                i64::try_from(identity.revision)
                    .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?,
                identity.verified_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::DuplicateProject, source)
        })?;
    Ok(())
}

fn update_project_identity_row(
    connection: &Connection,
    identity: &ProjectFilesystemIdentityRecord,
) -> Result<(), RepositoryError> {
    let changed = connection
        .execute(
            "UPDATE project_filesystem_identities
             SET repository_kind = ?2,
                 git_common_volume_serial_hex = ?3,
                 git_common_file_id_hex = ?4,
                 confirmed = ?5,
                 revision = ?6,
                 verified_at_ms = ?7
             WHERE project_id = ?1 AND revision < ?6",
            params![
                identity.project_id.to_string(),
                repository_kind_text(identity.repository_kind),
                identity.git_common_volume_serial_hex,
                identity.git_common_file_id_hex,
                i64::from(identity.confirmed),
                i64::try_from(identity.revision)
                    .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?,
                identity.verified_at_ms,
            ],
        )
        .map_err(operation_failed)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(repository_error(RepositoryErrorCode::VersionConflict))
    }
}

fn insert_git_operation_attempt(
    connection: &Connection,
    isolation: &TaskGitIsolation,
    kind: GitOperationKind,
) -> Result<(), RepositoryError> {
    let operation_id = isolation
        .operation_id
        .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidAggregate))?;
    let revision: i64 = connection
        .query_row(
            "SELECT revision FROM project_filesystem_identities WHERE project_id = ?1",
            [isolation.project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(operation_failed)?;
    connection
        .execute(
            "INSERT INTO git_operation_attempts (
                operation_id, task_id, project_id, operation_kind, status,
                approved_task_version, project_identity_revision, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, 'IntentRecorded', ?5, ?6, ?7, ?7)",
            params![
                operation_id.to_string(),
                isolation.task_id.to_string(),
                isolation.project_id.to_string(),
                operation_kind_text(kind),
                to_sql_integer(isolation.expected_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                revision,
                isolation.updated_at_ms,
            ],
        )
        .map_err(operation_failed)?;
    Ok(())
}

fn insert_operation_receipt(
    connection: &Connection,
    operation_id: GitOperationId,
    kind: GitOperationReceiptKind,
    evidence: Option<&str>,
    recorded_at_ms: i64,
) -> Result<(), RepositoryError> {
    let next: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM git_operation_receipts WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| row.get(0),
        )
        .map_err(operation_failed)?;
    connection
        .execute(
            "INSERT INTO git_operation_receipts (
                operation_id, sequence, receipt_kind, evidence, recorded_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                operation_id.to_string(),
                next,
                receipt_kind_text(kind),
                evidence,
                recorded_at_ms
            ],
        )
        .map_err(operation_failed)?;
    Ok(())
}

fn complete_operation_attempt(
    connection: &Connection,
    isolation: &TaskGitIsolation,
    status: &str,
    receipt: GitOperationReceiptKind,
) -> Result<(), RepositoryError> {
    let operation_id = isolation
        .operation_id
        .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidAggregate))?;
    let changed = connection
        .execute(
            "UPDATE git_operation_attempts
             SET status = ?2, updated_at_ms = ?3
             WHERE operation_id = ?1 AND status = 'IntentRecorded'",
            params![operation_id.to_string(), status, isolation.updated_at_ms],
        )
        .map_err(operation_failed)?;
    if changed != 1 {
        return Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        ));
    }
    insert_operation_receipt(
        connection,
        operation_id,
        receipt,
        None,
        isolation.updated_at_ms,
    )
}

fn load_operation_receipts(
    connection: &Connection,
    operation_id: GitOperationId,
) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, receipt_kind, evidence, recorded_at_ms
             FROM git_operation_receipts
             WHERE operation_id = ?1 ORDER BY sequence",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map([operation_id.to_string()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(operation_failed)?;
    let mut receipts = Vec::new();
    for row in rows {
        let (sequence, kind, evidence, recorded_at_ms) = row.map_err(operation_failed)?;
        receipts.push(GitOperationReceipt {
            operation_id,
            sequence: u64::try_from(sequence)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            kind: parse_receipt_kind(&kind)?,
            evidence,
            recorded_at_ms,
        });
    }
    Ok(receipts)
}

fn load_incomplete_operations(
    connection: &Connection,
) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT operation_id, task_id, project_id, operation_kind, status,
                    approved_task_version, project_identity_revision, created_at_ms, updated_at_ms
             FROM git_operation_attempts
             WHERE status = 'IntentRecorded'
             ORDER BY created_at_ms, operation_id",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(operation_failed)?;
    let mut attempts = Vec::new();
    for row in rows {
        let (operation, task, project, kind, status, version, revision, created, updated) =
            row.map_err(operation_failed)?;
        attempts.push(GitOperationAttempt {
            operation_id: GitOperationId::from_str(&operation)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            task_id: TaskId::from_str(&task)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            project_id: ProjectId::from_str(&project)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            operation_kind: parse_operation_kind(&kind)?,
            status: parse_attempt_status(&status)?,
            approved_task_version: u64::try_from(version)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            project_identity_revision: u64::try_from(revision)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            created_at_ms: created,
            updated_at_ms: updated,
        });
    }
    Ok(attempts)
}

const fn operation_kind_text(kind: GitOperationKind) -> &'static str {
    match kind {
        GitOperationKind::GitInitialize => "GitInitialize",
        GitOperationKind::WorktreeCreate => "WorktreeCreate",
    }
}

const fn receipt_kind_text(kind: GitOperationReceiptKind) -> &'static str {
    match kind {
        GitOperationReceiptKind::CommandStarted => "CommandStarted",
        GitOperationReceiptKind::CommandSucceeded => "CommandSucceeded",
        GitOperationReceiptKind::PostVerified => "PostVerified",
        GitOperationReceiptKind::CompletionRecorded => "CompletionRecorded",
        GitOperationReceiptKind::RecoveryRequired => "RecoveryRequired",
    }
}

fn parse_operation_kind(value: &str) -> Result<GitOperationKind, RepositoryError> {
    match value {
        "GitInitialize" => Ok(GitOperationKind::GitInitialize),
        "WorktreeCreate" => Ok(GitOperationKind::WorktreeCreate),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn parse_attempt_status(value: &str) -> Result<GitOperationAttemptStatus, RepositoryError> {
    match value {
        "IntentRecorded" => Ok(GitOperationAttemptStatus::IntentRecorded),
        "RecoveryRequired" => Ok(GitOperationAttemptStatus::RecoveryRequired),
        "Completed" => Ok(GitOperationAttemptStatus::Completed),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn parse_receipt_kind(value: &str) -> Result<GitOperationReceiptKind, RepositoryError> {
    match value {
        "CommandStarted" => Ok(GitOperationReceiptKind::CommandStarted),
        "CommandSucceeded" => Ok(GitOperationReceiptKind::CommandSucceeded),
        "PostVerified" => Ok(GitOperationReceiptKind::PostVerified),
        "CompletionRecorded" => Ok(GitOperationReceiptKind::CompletionRecorded),
        "RecoveryRequired" => Ok(GitOperationReceiptKind::RecoveryRequired),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

const fn repository_kind_text(kind: RepositoryKind) -> &'static str {
    match kind {
        RepositoryKind::Git => "Git",
        RepositoryKind::NonGit => "NonGit",
    }
}

fn validate_new_isolation_task(
    task: &Task,
    initial: &TaskStateTransition,
    classified: &TaskStateTransition,
    lease_acquired_at_ms: i64,
    isolation: &TaskGitIsolation,
) -> Result<(), RepositoryError> {
    task.validate_invariants()
        .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
    validate_nonnegative_task(task)?;
    let expected_isolation_status = match task.state() {
        TaskState::ProjectValidated => GitIsolationStatus::Ready,
        TaskState::AwaitingGitInitApproval => GitIsolationStatus::AwaitingGitInitApproval,
        _ => return Err(repository_error(RepositoryErrorCode::InvalidAggregate)),
    };
    if task.version() != 1
        || !task.state().requires_active_lease()
        || initial.task_id() != task.id()
        || initial.sequence() != 1
        || initial.from_state().is_some()
        || initial.to_state() != TaskState::Created
        || initial.task_version() != 0
        || classified.task_id() != task.id()
        || classified.sequence() != 2
        || classified.from_state() != Some(TaskState::Created)
        || classified.to_state() != task.state()
        || classified.task_version() != 1
        || classified.occurred_at_ms() != task.updated_at_ms()
        || initial.occurred_at_ms() < task.created_at_ms()
        || classified.occurred_at_ms() < initial.occurred_at_ms()
        || lease_acquired_at_ms < 0
        || isolation.task_id != task.id()
        || isolation.project_id != task.project_id()
        || isolation.status != expected_isolation_status
        || isolation.operation_id.is_some()
        || isolation.expected_task_version != task.version()
        || isolation.base_branch.is_some()
        || isolation.base_commit.is_some()
        || isolation.worktree_path.is_some()
        || isolation.branch_created_by_app
        || isolation.worktree_created_by_app
        || isolation.created_at_ms < 0
        || isolation.updated_at_ms < isolation.created_at_ms
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_isolation(
    connection: &Connection,
    isolation: &TaskGitIsolation,
) -> Result<(), RepositoryError> {
    validate_isolation_shape(isolation)?;
    connection
        .execute(
            "INSERT INTO task_git_isolations (
                task_id, project_id, status, operation_id, expected_task_version,
                base_branch, base_commit, worktree_path, branch_created_by_app,
                worktree_created_by_app, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                isolation.task_id.to_string(),
                isolation.project_id.to_string(),
                isolation_status_text(isolation.status),
                isolation.operation_id.map(|value| value.to_string()),
                to_sql_integer(isolation.expected_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                isolation.base_branch,
                isolation.base_commit,
                isolation.worktree_path,
                i64::from(isolation.branch_created_by_app),
                i64::from(isolation.worktree_created_by_app),
                isolation.created_at_ms,
                isolation.updated_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::DuplicateIsolation, source)
        })?;
    Ok(())
}

fn update_isolation(
    connection: &Connection,
    isolation: &TaskGitIsolation,
) -> Result<(), RepositoryError> {
    validate_isolation_shape(isolation)?;
    let changed = connection
        .execute(
            "UPDATE task_git_isolations
             SET status = ?1,
                 operation_id = ?2,
                 expected_task_version = ?3,
                 base_branch = ?4,
                 base_commit = ?5,
                 worktree_path = ?6,
                 branch_created_by_app = ?7,
                 worktree_created_by_app = ?8,
                 updated_at_ms = ?9
             WHERE task_id = ?10 AND project_id = ?11",
            params![
                isolation_status_text(isolation.status),
                isolation.operation_id.map(|value| value.to_string()),
                to_sql_integer(isolation.expected_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                isolation.base_branch,
                isolation.base_commit,
                isolation.worktree_path,
                i64::from(isolation.branch_created_by_app),
                i64::from(isolation.worktree_created_by_app),
                isolation.updated_at_ms,
                isolation.task_id.to_string(),
                isolation.project_id.to_string(),
            ],
        )
        .map_err(operation_failed)?;
    if changed != 1 {
        return Err(repository_error(RepositoryErrorCode::IsolationNotFound));
    }
    Ok(())
}

fn validate_isolation_shape(isolation: &TaskGitIsolation) -> Result<(), RepositoryError> {
    let empty_base = isolation.base_branch.is_none()
        && isolation.base_commit.is_none()
        && isolation.worktree_path.is_none();
    let complete_base = isolation
        .base_branch
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        && isolation.base_commit.as_deref().is_some_and(|value| {
            is_lower_hex(value, value.len()) && matches!(value.len(), 40 | 64)
        })
        && isolation
            .worktree_path
            .as_deref()
            .is_some_and(|value| !value.is_empty());
    let no_ownership = !isolation.branch_created_by_app && !isolation.worktree_created_by_app;
    let valid = match isolation.status {
        GitIsolationStatus::AwaitingGitInitApproval => {
            isolation.operation_id.is_none() && empty_base && no_ownership
        }
        GitIsolationStatus::Ready => empty_base && no_ownership,
        GitIsolationStatus::GitInitInProgress => {
            isolation.operation_id.is_some() && empty_base && no_ownership
        }
        GitIsolationStatus::WorktreeCreating => {
            isolation.operation_id.is_some() && complete_base && no_ownership
        }
        GitIsolationStatus::WorktreeReady => {
            isolation.operation_id.is_some()
                && complete_base
                && isolation.branch_created_by_app
                && isolation.worktree_created_by_app
        }
        GitIsolationStatus::RecoveryRequired => {
            isolation.operation_id.is_some() && (empty_base || complete_base) && no_ownership
        }
    };
    if valid && isolation.created_at_ms >= 0 && isolation.updated_at_ms >= isolation.created_at_ms {
        Ok(())
    } else {
        Err(repository_error(RepositoryErrorCode::InvalidAggregate))
    }
}

fn validate_isolation_expected_version(
    connection: &Connection,
    task_id: TaskId,
    expected_version: u64,
) -> Result<(), RepositoryError> {
    let task = load_task(connection, task_id)?
        .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
    if task.version() != expected_version {
        return Err(repository_error(RepositoryErrorCode::VersionConflict));
    }
    if load_isolation(connection, task_id)?.is_none() {
        return Err(repository_error(RepositoryErrorCode::IsolationNotFound));
    }
    Ok(())
}

fn persist_isolation_transition(
    connection: &Connection,
    expected_version: u64,
    task: &Task,
    transition: &TaskStateTransition,
    isolation: &TaskGitIsolation,
    terminal: bool,
) -> Result<(), RepositoryError> {
    task.validate_invariants()
        .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
    validate_nonnegative_task(task)?;
    if task.state().is_terminal() != terminal
        || isolation.task_id != task.id()
        || isolation.project_id != task.project_id()
        || isolation.expected_task_version != task.version()
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    let current = load_task(connection, task.id())?
        .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
    validate_transition_persistence(connection, expected_version, &current, task, transition)?;
    if query_active_lease(connection)?
        .as_ref()
        .map(|active| active.task_id)
        != Some(task.id())
    {
        return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
    }
    let current_isolation = load_isolation(connection, task.id())?
        .ok_or_else(|| repository_error(RepositoryErrorCode::IsolationNotFound))?;
    if current_isolation.project_id != isolation.project_id
        || current_isolation.created_at_ms != isolation.created_at_ms
        || isolation.updated_at_ms < current_isolation.updated_at_ms
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    update_task(connection, expected_version, task)?;
    insert_transition(connection, transition).map_err(operation_failed)?;
    update_isolation(connection, isolation)?;
    if terminal {
        let deleted = connection
            .execute(
                "DELETE FROM active_task_leases WHERE task_id = ?1",
                [task.id().to_string()],
            )
            .map_err(operation_failed)?;
        if deleted != 1 {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
    }
    Ok(())
}

fn validate_new_task(
    task: &Task,
    transition: &TaskStateTransition,
    lease_acquired_at_ms: i64,
) -> Result<(), RepositoryError> {
    task.validate_invariants()
        .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
    validate_nonnegative_task(task)?;
    if task.state() != TaskState::Created
        || task.version() != 0
        || task.resume_target_state().is_some()
        || task.terminal_at_ms().is_some()
        || !task.state().requires_active_lease()
        || transition.task_id() != task.id()
        || transition.sequence() != 1
        || transition.from_state().is_some()
        || transition.to_state() != TaskState::Created
        || transition.task_version() != 0
        || transition.occurred_at_ms() < task.created_at_ms()
        || lease_acquired_at_ms < 0
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn validate_nonnegative_task(task: &Task) -> Result<(), RepositoryError> {
    if task.created_at_ms() < 0
        || task.updated_at_ms() < 0
        || task.terminal_at_ms().is_some_and(|timestamp| timestamp < 0)
    {
        Err(repository_error(RepositoryErrorCode::InvalidAggregate))
    } else {
        Ok(())
    }
}

fn validate_transition_persistence(
    connection: &Connection,
    expected_version: u64,
    current: &Task,
    task: &Task,
    transition: &TaskStateTransition,
) -> Result<(), RepositoryError> {
    if current.version() != expected_version {
        return Err(repository_error(RepositoryErrorCode::VersionConflict));
    }
    let next_version = expected_version
        .checked_add(1)
        .ok_or_else(|| repository_error(RepositoryErrorCode::VersionConflict))?;
    if task.version() != next_version {
        return Err(repository_error(RepositoryErrorCode::VersionConflict));
    }
    if task.id() != current.id()
        || task.project_id() != current.project_id()
        || task.task_branch_identity() != current.task_branch_identity()
        || task.created_at_ms() != current.created_at_ms()
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if task.updated_at_ms() < current.updated_at_ms()
        || transition.occurred_at_ms() < current.updated_at_ms()
        || transition.occurred_at_ms() != task.updated_at_ms()
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    validate_state_change_context(current, task)?;
    if transition.task_id() != task.id()
        || transition.from_state() != Some(current.state())
        || transition.to_state() != task.state()
        || transition.task_version() != task.version()
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }

    let previous_sequence = last_transition_sequence(connection, task.id())?;
    let expected_sequence = TaskStateTransition::checked_next_sequence(previous_sequence)
        .map_err(|_| repository_error(RepositoryErrorCode::TransitionSequenceConflict))?;
    if transition.sequence() != expected_sequence {
        return Err(repository_error(
            RepositoryErrorCode::TransitionSequenceConflict,
        ));
    }
    Ok(())
}

fn validate_state_change_context(current: &Task, task: &Task) -> Result<(), RepositoryError> {
    let from = current.state();
    let to = task.state();

    if from.can_transition_to(to) {
        return Ok(());
    }
    if !from.can_contextually_transition_to(to) {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }

    let valid_context = if from.can_pause() && to == TaskState::Paused {
        task.resume_target_state() == Some(from)
    } else if from == TaskState::Paused && to.is_resume_target() {
        current.resume_target_state() == Some(to) && task.resume_target_state().is_none()
    } else if from == TaskState::RecoveryRequired && to == TaskState::Paused {
        current.resume_target_state().is_some()
            && task.resume_target_state() == current.resume_target_state()
    } else if from == TaskState::RecoveryRequired && to.is_resume_target() {
        current.resume_target_state() == Some(to) && task.resume_target_state().is_none()
    } else {
        false
    };

    if valid_context {
        Ok(())
    } else {
        Err(repository_error(RepositoryErrorCode::InvalidAggregate))
    }
}

fn insert_task(connection: &Connection, task: &Task) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO tasks (
            id, project_id, state, version, task_branch_identity, resume_target_state,
            created_at_ms, updated_at_ms, terminal_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            task.id().to_string(),
            task.project_id().to_string(),
            state_text(task.state()),
            to_sql_integer(task.version()).map_err(to_sql_conversion_error)?,
            task.task_branch_identity().as_str(),
            task.resume_target_state().map(state_text),
            task.created_at_ms(),
            task.updated_at_ms(),
            task.terminal_at_ms()
        ],
    )
}

fn update_task(
    connection: &Connection,
    expected_version: u64,
    task: &Task,
) -> Result<(), RepositoryError> {
    let changed = connection
        .execute(
            "UPDATE tasks
             SET state = ?1,
                 version = ?2,
                 resume_target_state = ?3,
                 updated_at_ms = ?4,
                 terminal_at_ms = ?5
             WHERE id = ?6 AND version = ?7",
            params![
                state_text(task.state()),
                to_sql_integer(task.version())
                    .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?,
                task.resume_target_state().map(state_text),
                task.updated_at_ms(),
                task.terminal_at_ms(),
                task.id().to_string(),
                to_sql_integer(expected_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?
            ],
        )
        .map_err(operation_failed)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(repository_error(RepositoryErrorCode::VersionConflict))
    }
}

fn insert_transition(
    connection: &Connection,
    transition: &TaskStateTransition,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO task_state_transitions (
            id, task_id, sequence, from_state, to_state, task_version,
            actor_kind, reason_code, occurred_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            transition.id().to_string(),
            transition.task_id().to_string(),
            to_sql_integer(transition.sequence()).map_err(to_sql_conversion_error)?,
            transition.from_state().map(state_text),
            state_text(transition.to_state()),
            to_sql_integer(transition.task_version()).map_err(to_sql_conversion_error)?,
            transition.actor_kind().as_str(),
            transition.reason_code().as_str(),
            transition.occurred_at_ms()
        ],
    )
}

fn load_task(connection: &Connection, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT id, project_id, state, version, task_branch_identity,
                    resume_target_state, created_at_ms, updated_at_ms, terminal_at_ms
             FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| {
                Ok(TaskRow {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    state: row.get(2)?,
                    version: row.get(3)?,
                    branch_identity: row.get(4)?,
                    resume_target: row.get(5)?,
                    created_at_ms: row.get(6)?,
                    updated_at_ms: row.get(7)?,
                    terminal_at_ms: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(operation_failed)?;
    row.map(TaskRow::restore).transpose()
}

struct TaskRow {
    id: String,
    project_id: String,
    state: String,
    version: i64,
    branch_identity: String,
    resume_target: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    terminal_at_ms: Option<i64>,
}

impl TaskRow {
    fn restore(self) -> Result<Task, RepositoryError> {
        let id = TaskId::from_str(&self.id).map_err(invalid_persistence)?;
        let project_id = ProjectId::from_str(&self.project_id).map_err(invalid_persistence)?;
        let state = parse_state(&self.state)?;
        let version = u64::try_from(self.version)
            .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        let task_branch_identity =
            TaskBranchIdentity::from_str(&self.branch_identity).map_err(invalid_persistence)?;
        let resume_target_state = self.resume_target.as_deref().map(parse_state).transpose()?;
        Task::restore(TaskSnapshot {
            id,
            project_id,
            state,
            version,
            task_branch_identity,
            resume_target_state,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            terminal_at_ms: self.terminal_at_ms,
        })
        .map_err(invalid_persistence)
    }
}

fn load_and_validate_transitions(
    connection: &Connection,
    task_id: TaskId,
) -> Result<Vec<TaskStateTransition>, RepositoryError> {
    let task = load_task(connection, task_id)?
        .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
    let mut statement = connection
        .prepare(
            "SELECT id, task_id, sequence, from_state, to_state, task_version,
                    actor_kind, reason_code, occurred_at_ms
             FROM task_state_transitions
             WHERE task_id = ?1
             ORDER BY sequence",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map([task_id.to_string()], |row| {
            Ok(TransitionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                sequence: row.get(2)?,
                from_state: row.get(3)?,
                to_state: row.get(4)?,
                task_version: row.get(5)?,
                actor_kind: row.get(6)?,
                reason_code: row.get(7)?,
                occurred_at_ms: row.get(8)?,
            })
        })
        .map_err(operation_failed)?;
    let mut transitions = Vec::new();
    for row in rows {
        transitions.push(row.map_err(operation_failed)?.restore()?);
    }
    validate_transition_history(task_id, &task, &transitions)?;
    Ok(transitions)
}

struct TransitionRow {
    id: String,
    task_id: String,
    sequence: i64,
    from_state: Option<String>,
    to_state: String,
    task_version: i64,
    actor_kind: String,
    reason_code: String,
    occurred_at_ms: i64,
}

impl TransitionRow {
    fn restore(self) -> Result<TaskStateTransition, RepositoryError> {
        TaskStateTransition::new(TaskStateTransitionSnapshot {
            id: TaskStateTransitionId::from_str(&self.id).map_err(invalid_persistence)?,
            task_id: TaskId::from_str(&self.task_id).map_err(invalid_persistence)?,
            sequence: u64::try_from(self.sequence)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            from_state: self.from_state.as_deref().map(parse_state).transpose()?,
            to_state: parse_state(&self.to_state)?,
            task_version: u64::try_from(self.task_version)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            actor_kind: ActorKind::from_str(&self.actor_kind).map_err(invalid_persistence)?,
            reason_code: ReasonCode::from_str(&self.reason_code).map_err(invalid_persistence)?,
            occurred_at_ms: self.occurred_at_ms,
        })
        .map_err(invalid_persistence)
    }
}

fn validate_transition_history(
    task_id: TaskId,
    task: &Task,
    transitions: &[TaskStateTransition],
) -> Result<(), RepositoryError> {
    let first = transitions
        .first()
        .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
    if first.task_id() != task_id
        || first.sequence() != 1
        || first.from_state().is_some()
        || first.to_state() != TaskState::Created
        || first.task_version() != 0
        || first.occurred_at_ms() < task.created_at_ms()
    {
        return Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        ));
    }

    for pair in transitions.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let expected_sequence = previous
            .sequence()
            .checked_add(1)
            .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        let expected_version = previous
            .task_version()
            .checked_add(1)
            .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        if current.task_id() != task_id
            || current.sequence() != expected_sequence
            || current.task_version() != expected_version
            || current.from_state() != Some(previous.to_state())
            || current.occurred_at_ms() < previous.occurred_at_ms()
        {
            return Err(repository_error(
                RepositoryErrorCode::InvalidPersistenceState,
            ));
        }
    }

    let last = transitions.last().expect("first transition was checked");
    if last.task_version() != task.version() || last.to_state() != task.state() {
        return Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        ));
    }
    Ok(())
}

fn load_projects(connection: &Connection) -> Result<Vec<ProjectSummary>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, root_path, canonical_path_key, display_path,
                    created_at_ms, updated_at_ms
             FROM projects
             ORDER BY created_at_ms ASC, id ASC",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(operation_failed)?;
    let mut projects = Vec::new();
    for row in rows {
        let (id, name, root_path, canonical_path_key, display_path, created_at_ms, updated_at_ms) =
            row.map_err(operation_failed)?;
        if name.is_empty()
            || root_path.is_empty()
            || canonical_path_key.is_empty()
            || display_path.is_empty()
            || created_at_ms < 0
            || updated_at_ms < created_at_ms
        {
            return Err(repository_error(
                RepositoryErrorCode::InvalidPersistenceState,
            ));
        }
        projects.push(ProjectSummary {
            id: ProjectId::from_str(&id).map_err(invalid_persistence)?,
            name,
            root_path,
            canonical_path_key,
            display_path,
            created_at_ms,
            updated_at_ms,
        });
    }
    Ok(projects)
}

fn load_project(
    connection: &Connection,
    project_id: ProjectId,
) -> Result<Option<ProjectRecord>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT id, name, root_path, canonical_path_key, display_path,
                    created_at_ms, updated_at_ms
             FROM projects WHERE id = ?1",
            [project_id.to_string()],
            |row| {
                Ok(ProjectRecord {
                    id: ProjectId::from_str(&row.get::<_, String>(0)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    name: row.get(1)?,
                    root_path: row.get(2)?,
                    canonical_path_key: row.get(3)?,
                    display_path: row.get(4)?,
                    created_at_ms: row.get(5)?,
                    updated_at_ms: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(operation_failed)?;
    match row {
        Some(project) => {
            validate_project(&project)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
            Ok(Some(project))
        }
        None => Ok(None),
    }
}

fn load_project_identity(
    connection: &Connection,
    project_id: ProjectId,
) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT project_id, root_volume_serial_hex, root_file_id_hex,
                    repository_kind, git_common_volume_serial_hex, git_common_file_id_hex,
                    confirmed, revision, verified_at_ms
             FROM project_filesystem_identities WHERE project_id = ?1",
            [project_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()
        .map_err(operation_failed)?;
    row.map(
        |(
            project_id,
            root_volume,
            root_file,
            kind,
            common_volume,
            common_file,
            confirmed,
            revision,
            verified,
        )| {
            let identity = ProjectFilesystemIdentityRecord {
                project_id: ProjectId::from_str(&project_id).map_err(invalid_persistence)?,
                root_volume_serial_hex: root_volume,
                root_file_id_hex: root_file,
                repository_kind: match kind.as_str() {
                    "Git" => RepositoryKind::Git,
                    "NonGit" => RepositoryKind::NonGit,
                    _ => {
                        return Err(repository_error(
                            RepositoryErrorCode::InvalidPersistenceState,
                        ));
                    }
                },
                git_common_volume_serial_hex: common_volume,
                git_common_file_id_hex: common_file,
                confirmed: match confirmed {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(repository_error(
                            RepositoryErrorCode::InvalidPersistenceState,
                        ));
                    }
                },
                revision: u64::try_from(revision)
                    .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
                verified_at_ms: verified,
            };
            validate_project_identity(&identity)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
            Ok(identity)
        },
    )
    .transpose()
}

fn load_isolation(
    connection: &Connection,
    task_id: TaskId,
) -> Result<Option<TaskGitIsolation>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT task_id, project_id, status, operation_id, expected_task_version,
                    base_branch, base_commit, worktree_path, branch_created_by_app,
                    worktree_created_by_app, created_at_ms, updated_at_ms
             FROM task_git_isolations WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()
        .map_err(operation_failed)?;
    row.map(
        |(
            task_id,
            project_id,
            status,
            operation_id,
            expected_task_version,
            base_branch,
            base_commit,
            worktree_path,
            branch_created_by_app,
            worktree_created_by_app,
            created_at_ms,
            updated_at_ms,
        )| {
            if !matches!(branch_created_by_app, 0 | 1)
                || !matches!(worktree_created_by_app, 0 | 1)
                || expected_task_version < 0
                || created_at_ms < 0
                || updated_at_ms < created_at_ms
            {
                return Err(repository_error(
                    RepositoryErrorCode::InvalidPersistenceState,
                ));
            }
            let isolation = TaskGitIsolation {
                task_id: TaskId::from_str(&task_id).map_err(invalid_persistence)?,
                project_id: ProjectId::from_str(&project_id).map_err(invalid_persistence)?,
                status: parse_isolation_status(&status)?,
                operation_id: operation_id
                    .as_deref()
                    .map(GitOperationId::from_str)
                    .transpose()
                    .map_err(invalid_persistence)?,
                expected_task_version: u64::try_from(expected_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
                base_branch,
                base_commit,
                worktree_path,
                branch_created_by_app: branch_created_by_app == 1,
                worktree_created_by_app: worktree_created_by_app == 1,
                created_at_ms,
                updated_at_ms,
            };
            validate_isolation_shape(&isolation)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
            Ok(isolation)
        },
    )
    .transpose()
}

fn query_active_lease(connection: &Connection) -> Result<Option<ActiveLease>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT task_id, acquired_at_ms
             FROM active_task_leases
             WHERE singleton_key = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(operation_failed)?;
    row.map(|(task_id, acquired_at_ms)| {
        if acquired_at_ms < 0 {
            return Err(repository_error(
                RepositoryErrorCode::InvalidPersistenceState,
            ));
        }
        Ok(ActiveLease {
            task_id: TaskId::from_str(&task_id).map_err(invalid_persistence)?,
            acquired_at_ms,
        })
    })
    .transpose()
}

fn project_exists(connection: &Connection, project_id: ProjectId) -> Result<bool, RepositoryError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(operation_failed)
}

fn task_exists(connection: &Connection, task_id: TaskId) -> Result<bool, RepositoryError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .map_err(operation_failed)
}

fn last_transition_sequence(
    connection: &Connection,
    task_id: TaskId,
) -> Result<u64, RepositoryError> {
    let sequence: Option<i64> = connection
        .query_row(
            "SELECT MAX(sequence) FROM task_state_transitions WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .map_err(operation_failed)?;
    sequence
        .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidPersistenceState))
        .and_then(|value| {
            u64::try_from(value)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))
        })
}

fn to_sql_integer(value: u64) -> Result<i64, ()> {
    i64::try_from(value).map_err(|_| ())
}

fn to_sql_conversion_error(_: ()) -> rusqlite::Error {
    rusqlite::Error::IntegralValueOutOfRange(0, i64::MAX)
}

fn state_text(state: TaskState) -> &'static str {
    match state {
        TaskState::Created => "Created",
        TaskState::ProjectValidated => "ProjectValidated",
        TaskState::AwaitingGitInitApproval => "AwaitingGitInitApproval",
        TaskState::GitInitialized => "GitInitialized",
        TaskState::WorktreeCreating => "WorktreeCreating",
        TaskState::WorktreeReady => "WorktreeReady",
        TaskState::PlanningWithClaude => "PlanningWithClaude",
        TaskState::AwaitingDesignApproval => "AwaitingDesignApproval",
        TaskState::ImplementingWithCodex => "ImplementingWithCodex",
        TaskState::Testing => "Testing",
        TaskState::AutoFixing => "AutoFixing",
        TaskState::ReviewingWithClaude => "ReviewingWithClaude",
        TaskState::ReviewFixing => "ReviewFixing",
        TaskState::AwaitingUserDiffApproval => "AwaitingUserDiffApproval",
        TaskState::Merging => "Merging",
        TaskState::MergeConflict => "MergeConflict",
        TaskState::PostMergeTesting => "PostMergeTesting",
        TaskState::Completed => "Completed",
        TaskState::Paused => "Paused",
        TaskState::Failed => "Failed",
        TaskState::RecoveryRequired => "RecoveryRequired",
        TaskState::UnknownExternalEffect => "UnknownExternalEffect",
        TaskState::Cancelled => "Cancelled",
        TaskState::CleanupPending => "CleanupPending",
        TaskState::Archived => "Archived",
    }
}

fn isolation_status_text(status: GitIsolationStatus) -> &'static str {
    match status {
        GitIsolationStatus::AwaitingGitInitApproval => "AwaitingGitInitApproval",
        GitIsolationStatus::Ready => "Ready",
        GitIsolationStatus::GitInitInProgress => "GitInitInProgress",
        GitIsolationStatus::WorktreeCreating => "WorktreeCreating",
        GitIsolationStatus::WorktreeReady => "WorktreeReady",
        GitIsolationStatus::RecoveryRequired => "RecoveryRequired",
    }
}

fn parse_isolation_status(value: &str) -> Result<GitIsolationStatus, RepositoryError> {
    match value {
        "AwaitingGitInitApproval" => Ok(GitIsolationStatus::AwaitingGitInitApproval),
        "Ready" => Ok(GitIsolationStatus::Ready),
        "GitInitInProgress" => Ok(GitIsolationStatus::GitInitInProgress),
        "WorktreeCreating" => Ok(GitIsolationStatus::WorktreeCreating),
        "WorktreeReady" => Ok(GitIsolationStatus::WorktreeReady),
        "RecoveryRequired" => Ok(GitIsolationStatus::RecoveryRequired),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn parse_state(value: &str) -> Result<TaskState, RepositoryError> {
    match value {
        "Created" => Ok(TaskState::Created),
        "ProjectValidated" => Ok(TaskState::ProjectValidated),
        "AwaitingGitInitApproval" => Ok(TaskState::AwaitingGitInitApproval),
        "GitInitialized" => Ok(TaskState::GitInitialized),
        "WorktreeCreating" => Ok(TaskState::WorktreeCreating),
        "WorktreeReady" => Ok(TaskState::WorktreeReady),
        "PlanningWithClaude" => Ok(TaskState::PlanningWithClaude),
        "AwaitingDesignApproval" => Ok(TaskState::AwaitingDesignApproval),
        "ImplementingWithCodex" => Ok(TaskState::ImplementingWithCodex),
        "Testing" => Ok(TaskState::Testing),
        "AutoFixing" => Ok(TaskState::AutoFixing),
        "ReviewingWithClaude" => Ok(TaskState::ReviewingWithClaude),
        "ReviewFixing" => Ok(TaskState::ReviewFixing),
        "AwaitingUserDiffApproval" => Ok(TaskState::AwaitingUserDiffApproval),
        "Merging" => Ok(TaskState::Merging),
        "MergeConflict" => Ok(TaskState::MergeConflict),
        "PostMergeTesting" => Ok(TaskState::PostMergeTesting),
        "Completed" => Ok(TaskState::Completed),
        "Paused" => Ok(TaskState::Paused),
        "Failed" => Ok(TaskState::Failed),
        "RecoveryRequired" => Ok(TaskState::RecoveryRequired),
        "UnknownExternalEffect" => Ok(TaskState::UnknownExternalEffect),
        "Cancelled" => Ok(TaskState::Cancelled),
        "CleanupPending" => Ok(TaskState::CleanupPending),
        "Archived" => Ok(TaskState::Archived),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn repository_error(code: RepositoryErrorCode) -> RepositoryError {
    RepositoryError::new(code)
}

fn invalid_persistence(error: impl std::error::Error + Send + Sync + 'static) -> RepositoryError {
    RepositoryError::with_source(RepositoryErrorCode::InvalidPersistenceState, error)
}

fn database_unavailable(error: rusqlite::Error) -> RepositoryError {
    RepositoryError::with_source(RepositoryErrorCode::DatabaseUnavailable, error)
}

fn operation_failed(error: rusqlite::Error) -> RepositoryError {
    RepositoryError::with_source(RepositoryErrorCode::OperationFailed, error)
}
