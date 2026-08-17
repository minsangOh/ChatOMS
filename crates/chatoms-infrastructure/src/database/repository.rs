use std::str::FromStr;

use chatoms_domain::{
    ActorKind, ContextDataScope, GitOperationId, HighRiskCategory, ProjectId, ReasonCode, Task,
    TaskBranchIdentity, TaskId, TaskSnapshot, TaskState, TaskStateTransition,
    TaskStateTransitionId, TaskStateTransitionSnapshot, ValidationCommandKind,
    ValidationExecutionScope, WorkKind,
};
use chatoms_ports::diff::DiffContentHash;
use chatoms_ports::git::RepositoryKind;
use chatoms_ports::provider::ProviderKind;
use chatoms_ports::repository::{
    ActiveLease, AppProfileRecord, ContextPackageManifestRecord, ContextPackagePreparation,
    DiffApprovalRecord, FoundationRepository, GitInitApproval, GitIsolationStatus,
    GitOperationAttempt, GitOperationAttemptStatus, GitOperationKind, GitOperationReceipt,
    GitOperationReceiptKind, HighRiskApprovalRecord, ImplementationResultOutcome,
    PlanningResultOutcome, PostMergeValidationResultAttempt, PostMergeValidationResultOutcome,
    PostMergeValidationResultRecord, ProjectFilesystemIdentityRecord, ProjectRecord,
    ProjectSummary, ProviderBindingRecord, ProviderConsent, RepositoryError, RepositoryErrorCode,
    ReviewResultOutcome, TaskBriefRecord, TaskGitIsolation, TaskImplementationResultRecord,
    TaskPlanningResultRecord, TaskReviewResultRecord, ValidationCommandApprovalRecord,
    ValidationCommandResultAttempt, ValidationCommandResultOutcome, ValidationCommandResultRecord,
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
        brief: Option<&TaskBriefRecord>,
    ) -> Result<(), RepositoryError> {
        validate_new_isolation_task(
            task,
            initial_transition,
            classified_transition,
            lease_acquired_at_ms,
            isolation,
        )?;
        if let Some(brief) = brief {
            validate_task_brief(brief)?;
            if brief.task_id != task.id() {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
        }
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
        if let Some(brief) = brief {
            insert_task_brief(&transaction, brief)?;
        }
        transaction.commit().map_err(operation_failed)
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        load_isolation(self.database.raw_mut(), task_id)
    }

    fn get_task_brief(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskBriefRecord>, RepositoryError> {
        load_task_brief(self.database.raw_mut(), task_id)
    }

    fn get_task_planning_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskPlanningResultRecord>, RepositoryError> {
        load_planning_result(self.database.raw_mut(), task_id)
    }

    fn get_task_implementation_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskImplementationResultRecord>, RepositoryError> {
        load_implementation_result(self.database.raw_mut(), task_id)
    }

    fn get_task_review_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskReviewResultRecord>, RepositoryError> {
        load_review_result(self.database.raw_mut(), task_id)
    }

    fn get_provider_consent(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        load_provider_consent(
            self.database.raw_mut(),
            task_id,
            provider,
            work_kind,
            approved_task_version,
            data_scope,
        )
    }

    fn save_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        if task.state() != TaskState::Planning {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        if let Some(consent) = consent
            && (consent.task_id != task.id() || consent.approved_task_version != expected_version)
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
        if current.state() != TaskState::WorktreeReady {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        // Consent is inserted before the task row is bumped to the next
        // version: the binding trigger requires tasks.version to still equal
        // approved_task_version (the pre-transition WorktreeReady version).
        if let Some(consent) = consent {
            insert_provider_consent(&transaction, consent)?;
        }
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_context_package_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        if task.state() != TaskState::Planning {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::WorktreeReady {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let isolation = load_isolation(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::IsolationNotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        // Re-verify inside this transaction that the Context Package v1
        // Planning consent/manifest pair actually exists — never trust the
        // caller's earlier read-only check alone. Both absent is a normal
        // "not prepared yet" precondition failure; exactly one present is
        // the already-corrupted invariant `prepare_planning_context_package`
        // also guards against.
        let consent = load_provider_consent(
            &transaction,
            task.id(),
            ProviderKind::Claude,
            WorkKind::Planning,
            expected_version,
            ContextDataScope::ContextPackageV1,
        )?;
        let manifest = load_context_package_manifest(
            &transaction,
            task.id(),
            ProviderKind::Claude,
            WorkKind::Planning,
            expected_version,
            ContextDataScope::ContextPackageV1,
        )?;
        match (consent, manifest) {
            (Some(_), Some(_)) => {}
            (None, None) => return Err(repository_error(RepositoryErrorCode::InvalidAggregate)),
            (Some(_), None) | (None, Some(_)) => {
                return Err(repository_error(
                    RepositoryErrorCode::InvalidPersistenceState,
                ));
            }
        }
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        if task.state() != TaskState::Implementing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        if let Some(consent) = consent
            && (consent.task_id != task.id()
                || consent.work_kind != WorkKind::Implementation
                || consent.approved_task_version != expected_version)
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
        if current.state() != TaskState::AwaitingDesignApproval {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        // Consent is inserted before the task row is bumped to the next
        // version: the binding trigger requires tasks.version to still equal
        // approved_task_version (the pre-transition AwaitingDesignApproval
        // version).
        if let Some(consent) = consent {
            insert_provider_consent(&transaction, consent)?;
        }
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_context_package_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        if task.state() != TaskState::Implementing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::AwaitingDesignApproval {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let isolation = load_isolation(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::IsolationNotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        // Re-verify inside this transaction that a Completed, non-empty
        // Claude Planning result is already stored — never trust the
        // caller's earlier read-only check alone. `task_planning_results` is
        // an immutable, insert-only table (no UPDATE/DELETE path exists), so
        // this can only ever have been true or false at check time and
        // cannot flip in between; re-checking it here is defense-in-depth
        // consistent with the consent/manifest re-check below, not a
        // response to a real race.
        let planning_result = load_planning_result(&transaction, task.id())?;
        let plan_text_present = planning_result
            .as_ref()
            .map(|result| {
                result.outcome == PlanningResultOutcome::Completed
                    && result
                        .plan_text
                        .as_deref()
                        .is_some_and(|text| !text.is_empty())
            })
            .unwrap_or(false);
        if !plan_text_present {
            return Err(repository_error(
                RepositoryErrorCode::InvalidPersistenceState,
            ));
        }
        // Re-verify inside this transaction that the Context Package v1
        // Implementation consent/manifest pair actually exists — never
        // trust the caller's earlier read-only check alone. Both absent is
        // a normal "not prepared yet" precondition failure; exactly one
        // present is the already-corrupted invariant
        // `prepare_implementation_context_package` also guards against.
        let consent = load_provider_consent(
            &transaction,
            task.id(),
            ProviderKind::Claude,
            WorkKind::Implementation,
            expected_version,
            ContextDataScope::ContextPackageV1,
        )?;
        let manifest = load_context_package_manifest(
            &transaction,
            task.id(),
            ProviderKind::Claude,
            WorkKind::Implementation,
            expected_version,
            ContextDataScope::ContextPackageV1,
        )?;
        match (consent, manifest) {
            (Some(_), Some(_)) => {}
            (None, None) => return Err(repository_error(RepositoryErrorCode::InvalidAggregate)),
            (Some(_), None) | (None, Some(_)) => {
                return Err(repository_error(
                    RepositoryErrorCode::InvalidPersistenceState,
                ));
            }
        }
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_review_consent(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        data_scope: ContextDataScope,
        consented_at_ms: i64,
    ) -> Result<ProviderConsent, RepositoryError> {
        if consented_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::Reviewing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let existing = load_provider_consent(
            &transaction,
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
            data_scope,
        )?;
        let consent = match existing {
            Some(consent) => consent,
            None => {
                let consent = ProviderConsent {
                    task_id,
                    provider: ProviderKind::Claude,
                    work_kind: WorkKind::Review,
                    approved_task_version: expected_version,
                    data_scope,
                    consented_at_ms,
                };
                insert_provider_consent(&transaction, &consent)?;
                consent
            }
        };
        transaction.commit().map_err(operation_failed)?;
        Ok(consent)
    }

    fn prepare_planning_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        if prepared_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::WorktreeReady {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let isolation = load_isolation(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::IsolationNotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let preparation = prepare_context_package(
            &transaction,
            task_id,
            WorkKind::Planning,
            expected_version,
            prepared_at_ms,
        )?;
        transaction.commit().map_err(operation_failed)?;
        Ok(preparation)
    }

    fn prepare_implementation_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        if prepared_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::AwaitingDesignApproval {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let preparation = prepare_context_package(
            &transaction,
            task_id,
            WorkKind::Implementation,
            expected_version,
            prepared_at_ms,
        )?;
        transaction.commit().map_err(operation_failed)?;
        Ok(preparation)
    }

    fn prepare_review_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        if prepared_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::Reviewing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let preparation = prepare_context_package(
            &transaction,
            task_id,
            WorkKind::Review,
            expected_version,
            prepared_at_ms,
        )?;
        transaction.commit().map_err(operation_failed)?;
        Ok(preparation)
    }

    fn save_context_package_manifest(
        &mut self,
        record: &ContextPackageManifestRecord,
    ) -> Result<(), RepositoryError> {
        validate_context_package_manifest_shape(record)?;
        insert_context_package_manifest(self.database.raw_mut(), record)
    }

    fn get_context_package_manifest(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ContextPackageManifestRecord>, RepositoryError> {
        load_context_package_manifest(
            self.database.raw_mut(),
            task_id,
            provider,
            work_kind,
            approved_task_version,
            data_scope,
        )
    }

    fn save_high_risk_approval(
        &mut self,
        approval: &HighRiskApprovalRecord,
    ) -> Result<(), RepositoryError> {
        validate_high_risk_approval_shape(approval)?;
        insert_high_risk_approval(self.database.raw_mut(), approval)
    }

    fn get_high_risk_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        load_high_risk_approval(
            self.database.raw_mut(),
            task_id,
            approved_task_version,
            risk_category,
        )
    }

    fn ensure_high_risk_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
        approved_at_ms: i64,
    ) -> Result<HighRiskApprovalRecord, RepositoryError> {
        if approved_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        let existing =
            load_high_risk_approval(&transaction, task_id, expected_version, risk_category)?;
        let approval = match existing {
            Some(approval) => approval,
            None => {
                let approval = HighRiskApprovalRecord {
                    task_id,
                    approved_task_version: expected_version,
                    risk_category,
                    approved_at_ms,
                };
                insert_high_risk_approval(&transaction, &approval)?;
                approval
            }
        };
        transaction.commit().map_err(operation_failed)?;
        Ok(approval)
    }

    fn save_diff_approval(&mut self, approval: &DiffApprovalRecord) -> Result<(), RepositoryError> {
        validate_diff_approval_shape(approval)?;
        insert_diff_approval(self.database.raw_mut(), approval)
    }

    fn get_diff_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        diff_content_hash: DiffContentHash,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        load_diff_approval(
            self.database.raw_mut(),
            task_id,
            approved_task_version,
            diff_content_hash,
        )
    }

    fn ensure_diff_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        diff_content_hash: DiffContentHash,
        approved_at_ms: i64,
    ) -> Result<DiffApprovalRecord, RepositoryError> {
        if approved_at_ms < 0 {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(repository_error(RepositoryErrorCode::VersionConflict));
        }
        let existing =
            load_diff_approval(&transaction, task_id, expected_version, diff_content_hash)?;
        let approval = match existing {
            Some(approval) => approval,
            None => {
                let approval = DiffApprovalRecord {
                    task_id,
                    approved_task_version: expected_version,
                    diff_content_hash,
                    approved_at_ms,
                };
                insert_diff_approval(&transaction, &approval)?;
                approval
            }
        };
        transaction.commit().map_err(operation_failed)?;
        Ok(approval)
    }

    fn save_planning_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskPlanningResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        if task.state().is_terminal() != terminal || result.task_id != task.id() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_planning_result_shape(result)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::Planning {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        insert_planning_result(&transaction, result)?;
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        if terminal {
            let deleted = transaction
                .execute(
                    "DELETE FROM active_task_leases WHERE task_id = ?1",
                    [task.id().to_string()],
                )
                .map_err(operation_failed)?;
            if deleted != 1 {
                return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
            }
        }
        transaction.commit().map_err(operation_failed)
    }

    fn save_implementation_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskImplementationResultRecord,
    ) -> Result<(), RepositoryError> {
        if task.state().is_terminal() || result.task_id != task.id() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_implementation_result_shape(result)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::Implementing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        insert_implementation_result(&transaction, result)?;
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn save_review_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskReviewResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        if task.state().is_terminal() != terminal || result.task_id != task.id() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_review_result_shape(result)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::Reviewing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        insert_review_result(&transaction, result)?;
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        if terminal {
            let deleted = transaction
                .execute(
                    "DELETE FROM active_task_leases WHERE task_id = ?1",
                    [task.id().to_string()],
                )
                .map_err(operation_failed)?;
            if deleted != 1 {
                return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
            }
        }
        transaction.commit().map_err(operation_failed)
    }

    fn save_validation_command_approval(
        &mut self,
        approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), RepositoryError> {
        validate_validation_command_approval_shape(approval)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, approval.task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        let state_matches_scope = match approval.execution_scope {
            ValidationExecutionScope::TaskWorktree => matches!(
                current.state(),
                TaskState::Implementing | TaskState::Testing
            ),
            ValidationExecutionScope::ProjectRoot => {
                current.state() == TaskState::AwaitingUserDiffApproval
            }
        };
        if current.version() != approval.approved_task_version || !state_matches_scope {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        if approval.execution_scope == ValidationExecutionScope::ProjectRoot {
            let identity = load_project_identity(&transaction, current.project_id())?
                .filter(|identity| identity.confirmed)
                .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidAggregate))?;
            if approval.target_project_id != Some(current.project_id())
                || approval.target_project_identity_revision != Some(identity.revision)
                || approval.target_root_volume_serial_hex.as_deref()
                    != Some(identity.root_volume_serial_hex.as_str())
                || approval.target_root_file_id_hex.as_deref()
                    != Some(identity.root_file_id_hex.as_str())
            {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
        }
        insert_validation_command_approval(&transaction, approval)?;
        transaction.commit().map_err(operation_failed)
    }

    fn list_validation_command_approvals(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        load_validation_command_approvals(
            self.database.raw_mut(),
            task_id,
            approved_task_version,
            ValidationExecutionScope::TaskWorktree,
        )
    }

    fn list_validation_command_approvals_for_scope(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        execution_scope: ValidationExecutionScope,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        load_validation_command_approvals(
            self.database.raw_mut(),
            task_id,
            approved_task_version,
            execution_scope,
        )
    }

    fn append_validation_command_result(
        &mut self,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<ValidationCommandResultRecord, RepositoryError> {
        validate_validation_command_result_attempt_shape(attempt)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        if attempt.execution_scope != ValidationExecutionScope::TaskWorktree {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let approval_exists = validation_command_approval_exists(
            &transaction,
            attempt.task_id,
            attempt.approved_task_version,
            attempt.execution_scope,
            attempt.kind,
        )?;
        if !approval_exists {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let attempt_sequence = next_validation_command_result_sequence(
            &transaction,
            attempt.task_id,
            attempt.approved_task_version,
            attempt.execution_scope,
            attempt.kind,
        )?;
        insert_validation_command_result(&transaction, attempt, attempt_sequence)?;
        transaction.commit().map_err(operation_failed)?;
        Ok(ValidationCommandResultRecord {
            task_id: attempt.task_id,
            approved_task_version: attempt.approved_task_version,
            execution_scope: attempt.execution_scope,
            kind: attempt.kind,
            attempt_sequence,
            outcome: attempt.outcome,
            exit_code: attempt.exit_code,
            safe_summary: attempt.safe_summary.clone(),
            started_at_ms: attempt.started_at_ms,
            completed_at_ms: attempt.completed_at_ms,
        })
    }

    fn list_validation_command_results(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<ValidationCommandResultRecord>, RepositoryError> {
        load_validation_command_results(
            self.database.raw_mut(),
            task_id,
            approved_task_version,
            kind,
        )
    }

    fn finalize_validation_command_batch(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<(), RepositoryError> {
        if task.state().is_terminal() || attempt.task_id != task.id() {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_validation_command_result_attempt_shape(attempt)?;
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::Testing {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        if attempt.execution_scope != ValidationExecutionScope::TaskWorktree {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let approval_exists = validation_command_approval_exists(
            &transaction,
            attempt.task_id,
            attempt.approved_task_version,
            attempt.execution_scope,
            attempt.kind,
        )?;
        if !approval_exists {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let attempt_sequence = next_validation_command_result_sequence(
            &transaction,
            attempt.task_id,
            attempt.approved_task_version,
            attempt.execution_scope,
            attempt.kind,
        )?;
        insert_validation_command_result(&transaction, attempt, attempt_sequence)?;
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        transaction.commit().map_err(operation_failed)
    }

    fn append_post_merge_validation_result(
        &mut self,
        attempt: &PostMergeValidationResultAttempt,
    ) -> Result<PostMergeValidationResultRecord, RepositoryError> {
        validate_post_merge_validation_attempt_shape(attempt)?;
        if attempt.outcome != PostMergeValidationResultOutcome::Success {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, attempt.task_id)?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::PostMergeTesting
            || current.version() != attempt.post_merge_task_version
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(attempt.task_id) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        ensure_post_merge_approval(&transaction, attempt)?;
        let attempt_sequence = next_post_merge_validation_result_sequence(&transaction, attempt)?;
        insert_post_merge_validation_result(&transaction, attempt, attempt_sequence)?;
        transaction.commit().map_err(operation_failed)?;
        Ok(post_merge_validation_record(attempt, attempt_sequence))
    }

    fn list_post_merge_validation_results(
        &mut self,
        task_id: TaskId,
        approval_task_version: u64,
        post_merge_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<PostMergeValidationResultRecord>, RepositoryError> {
        load_post_merge_validation_results(
            self.database.raw_mut(),
            task_id,
            approval_task_version,
            post_merge_task_version,
            kind,
        )
    }

    fn finalize_post_merge_validation_batch(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        attempt: &PostMergeValidationResultAttempt,
    ) -> Result<(), RepositoryError> {
        validate_post_merge_validation_attempt_shape(attempt)?;
        if attempt.task_id != task.id()
            || attempt.post_merge_task_version != expected_version
            || attempt.execution_scope != ValidationExecutionScope::ProjectRoot
        {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let expected_state = if attempt.outcome == PostMergeValidationResultOutcome::Success {
            TaskState::Completed
        } else {
            TaskState::RecoveryRequired
        };
        if task.state() != expected_state {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        let transaction = self
            .database
            .raw_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_unavailable)?;
        let current = load_task(&transaction, task.id())?
            .ok_or_else(|| repository_error(RepositoryErrorCode::TaskNotFound))?;
        if current.state() != TaskState::PostMergeTesting {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        validate_transition_persistence(
            &transaction,
            expected_version,
            &current,
            task,
            transition,
        )?;
        let lease = query_active_lease(&transaction)?;
        if lease.as_ref().map(|active| active.task_id) != Some(task.id()) {
            return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
        }
        ensure_post_merge_approval(&transaction, attempt)?;
        let attempt_sequence = next_post_merge_validation_result_sequence(&transaction, attempt)?;
        insert_post_merge_validation_result(&transaction, attempt, attempt_sequence)?;
        update_task(&transaction, expected_version, task)?;
        insert_transition(&transaction, transition).map_err(operation_failed)?;
        if task.state() == TaskState::Completed {
            let deleted = transaction
                .execute(
                    "DELETE FROM active_task_leases WHERE task_id = ?1",
                    [task.id().to_string()],
                )
                .map_err(operation_failed)?;
            if deleted != 1 {
                return Err(repository_error(RepositoryErrorCode::ActiveLeaseConflict));
            }
        }
        transaction.commit().map_err(operation_failed)
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

fn validate_task_brief(brief: &TaskBriefRecord) -> Result<(), RepositoryError> {
    if brief.requirements.trim().is_empty()
        || brief.completion_criteria.trim().is_empty()
        || brief.prohibited_scope.trim().is_empty()
        || brief.created_at_ms < 0
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_task_brief(
    connection: &Connection,
    brief: &TaskBriefRecord,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO task_briefs (
                task_id, requirements, completion_criteria, prohibited_scope, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                brief.task_id.to_string(),
                brief.requirements,
                brief.completion_criteria,
                brief.prohibited_scope,
                brief.created_at_ms,
            ],
        )
        .map_err(operation_failed)?;
    Ok(())
}

fn load_task_brief(
    connection: &Connection,
    task_id: TaskId,
) -> Result<Option<TaskBriefRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT task_id, requirements, completion_criteria, prohibited_scope, created_at_ms
             FROM task_briefs WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(operation_failed)?
        .map(
            |(id, requirements, completion_criteria, prohibited_scope, created_at_ms)| {
                Ok(TaskBriefRecord {
                    task_id: TaskId::from_str(&id).map_err(invalid_persistence)?,
                    requirements,
                    completion_criteria,
                    prohibited_scope,
                    created_at_ms,
                })
            },
        )
        .transpose()
}

fn insert_provider_consent(
    connection: &Connection,
    consent: &ProviderConsent,
) -> Result<(), RepositoryError> {
    if consent.consented_at_ms < 0 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    connection
        .execute(
            "INSERT INTO task_provider_consents (
                task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                consent.task_id.to_string(),
                provider_kind_text(consent.provider),
                work_kind_text(consent.work_kind),
                to_sql_integer(consent.approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                data_scope_text(consent.data_scope),
                consent.consented_at_ms,
            ],
        )
        .map_err(operation_failed)?;
    Ok(())
}

/// Looks up the consent row for the exact `(task_id, provider, work_kind,
/// approved_task_version, data_scope)` 5-tuple: `data_scope` is part of the
/// `WHERE` filter, never omitted or substituted. As defense in depth against
/// a corrupted or hand-edited database, the persisted `data_scope` text is
/// also read back and re-parsed via [`data_scope_from_text`] rather than
/// blindly trusted to equal the requested value; a mismatch or an
/// unrecognized value both fail closed as
/// `RepositoryErrorCode::InvalidPersistenceState` without the raw string
/// ever appearing in the error.
fn load_provider_consent(
    connection: &Connection,
    task_id: TaskId,
    provider: ProviderKind,
    work_kind: WorkKind,
    approved_task_version: u64,
    data_scope: ContextDataScope,
) -> Result<Option<ProviderConsent>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT data_scope, consented_at_ms FROM task_provider_consents
             WHERE task_id = ?1 AND provider = ?2 AND work_kind = ?3
               AND approved_task_version = ?4 AND data_scope = ?5",
            params![
                task_id.to_string(),
                provider_kind_text(provider),
                work_kind_text(work_kind),
                to_sql_integer(approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                data_scope_text(data_scope),
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(operation_failed)?;
    let Some((persisted_data_scope, consented_at_ms)) = row else {
        return Ok(None);
    };
    if data_scope_from_text(&persisted_data_scope)? != data_scope {
        return Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        ));
    }
    Ok(Some(ProviderConsent {
        task_id,
        provider,
        work_kind,
        approved_task_version,
        data_scope,
        consented_at_ms,
    }))
}

const fn work_kind_text(kind: WorkKind) -> &'static str {
    match kind {
        WorkKind::Planning => "Planning",
        WorkKind::Implementation => "Implementation",
        WorkKind::Review => "Review",
    }
}

const fn data_scope_text(scope: ContextDataScope) -> &'static str {
    match scope {
        ContextDataScope::LegacyPhase4 => "LegacyPhase4",
        ContextDataScope::ContextPackageV1 => "ContextPackageV1",
    }
}

fn data_scope_from_text(value: &str) -> Result<ContextDataScope, RepositoryError> {
    match value {
        "LegacyPhase4" => Ok(ContextDataScope::LegacyPhase4),
        "ContextPackageV1" => Ok(ContextDataScope::ContextPackageV1),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

/// Rejects a Context Package v1 manifest before it ever reaches the
/// database driver: `data_scope` must be exactly
/// [`ContextDataScope::ContextPackageV1`] (a manifest never exists for
/// [`ContextDataScope::LegacyPhase4`], which the SQL `CHECK` in
/// `0017_context_package_manifests.sql` also enforces independently) and
/// `created_at_ms` must be non-negative.
fn validate_context_package_manifest_shape(
    record: &ContextPackageManifestRecord,
) -> Result<(), RepositoryError> {
    if record.data_scope != ContextDataScope::ContextPackageV1 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if record.created_at_ms < 0 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_context_package_manifest(
    connection: &Connection,
    record: &ContextPackageManifestRecord,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO context_package_manifests (
                task_id, provider, work_kind, approved_task_version, data_scope, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.task_id.to_string(),
                provider_kind_text(record.provider),
                work_kind_text(record.work_kind),
                to_sql_integer(record.approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                data_scope_text(record.data_scope),
                record.created_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

/// Looks up the manifest row for the exact `(task_id, provider, work_kind,
/// approved_task_version, data_scope)` 5-tuple: `data_scope` is part of the
/// `WHERE` filter, never omitted or substituted. As defense in depth against
/// a corrupted or hand-edited database, the persisted `data_scope` text is
/// also read back and re-parsed via [`data_scope_from_text`] rather than
/// blindly trusted to equal the requested value, mirroring
/// [`load_provider_consent`]; a mismatch or an unrecognized value both fail
/// closed as `RepositoryErrorCode::InvalidPersistenceState` without the raw
/// string ever appearing in the error.
fn load_context_package_manifest(
    connection: &Connection,
    task_id: TaskId,
    provider: ProviderKind,
    work_kind: WorkKind,
    approved_task_version: u64,
    data_scope: ContextDataScope,
) -> Result<Option<ContextPackageManifestRecord>, RepositoryError> {
    let row = connection
        .query_row(
            "SELECT data_scope, created_at_ms FROM context_package_manifests
             WHERE task_id = ?1 AND provider = ?2 AND work_kind = ?3
               AND approved_task_version = ?4 AND data_scope = ?5",
            params![
                task_id.to_string(),
                provider_kind_text(provider),
                work_kind_text(work_kind),
                to_sql_integer(approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                data_scope_text(data_scope),
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(operation_failed)?;
    let Some((persisted_data_scope, created_at_ms)) = row else {
        return Ok(None);
    };
    if data_scope_from_text(&persisted_data_scope)? != data_scope {
        return Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        ));
    }
    Ok(Some(ContextPackageManifestRecord {
        task_id,
        provider,
        work_kind,
        approved_task_version,
        data_scope,
        created_at_ms,
    }))
}

fn validate_high_risk_approval_shape(
    approval: &HighRiskApprovalRecord,
) -> Result<(), RepositoryError> {
    if approval.approved_at_ms < 0 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_high_risk_approval(
    connection: &Connection,
    approval: &HighRiskApprovalRecord,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO task_high_risk_approvals (
                task_id, approved_task_version, risk_category, approved_at_ms
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                approval.task_id.to_string(),
                to_sql_integer(approval.approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                approval.risk_category.persisted_text(),
                approval.approved_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

/// Looks up the approval row for the exact `(task_id, approved_task_version,
/// risk_category)` identity. Scans every row recorded for `(task_id,
/// approved_task_version)` (at most 13, one per [`HighRiskCategory`]) and
/// parses each persisted `risk_category` via
/// [`HighRiskCategory::from_persisted_text`] rather than filtering on the
/// requested category as a raw SQL string — this way a corrupted or
/// hand-edited row for that task/version fails the whole lookup closed as
/// `RepositoryErrorCode::InvalidPersistenceState` (without the raw string
/// ever appearing in the error) even when a different, well-formed row for
/// the same task/version would otherwise have satisfied the request; a
/// corrupted table must never be silently treated as if only the
/// uncorrupted rows existed.
fn load_high_risk_approval(
    connection: &Connection,
    task_id: TaskId,
    approved_task_version: u64,
    risk_category: HighRiskCategory,
) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
    let version = to_sql_integer(approved_task_version)
        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?;
    let mut statement = connection
        .prepare(
            "SELECT risk_category, approved_at_ms FROM task_high_risk_approvals
             WHERE task_id = ?1 AND approved_task_version = ?2",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map(params![task_id.to_string(), version], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(operation_failed)?;
    let mut found_at_ms = None;
    for row in rows {
        let (persisted_category, approved_at_ms) = row.map_err(operation_failed)?;
        let parsed_category = HighRiskCategory::from_persisted_text(&persisted_category)
            .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        if parsed_category == risk_category {
            found_at_ms = Some(approved_at_ms);
        }
    }
    Ok(found_at_ms.map(|approved_at_ms| HighRiskApprovalRecord {
        task_id,
        approved_task_version,
        risk_category,
        approved_at_ms,
    }))
}

fn validate_diff_approval_shape(approval: &DiffApprovalRecord) -> Result<(), RepositoryError> {
    if approval.approved_at_ms < 0 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_diff_approval(
    connection: &Connection,
    approval: &DiffApprovalRecord,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO task_diff_approvals (
                task_id, approved_task_version, diff_content_hash_hex, approved_at_ms
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                approval.task_id.to_string(),
                to_sql_integer(approval.approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                approval.diff_content_hash.to_hex(),
                approval.approved_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

/// Looks up the approval row for the exact `(task_id, approved_task_version,
/// diff_content_hash)` identity. Scans every row recorded for `(task_id,
/// approved_task_version)` and parses each persisted
/// `diff_content_hash_hex` via [`DiffContentHash::from_hex`] rather than
/// filtering on the requested hash as a raw SQL string — mirroring
/// [`load_high_risk_approval`]'s reasoning exactly: this way a corrupted or
/// hand-edited row for that task/version fails the whole lookup closed as
/// `RepositoryErrorCode::InvalidPersistenceState` (without the raw string
/// ever appearing in the error) even when a different, well-formed row for
/// the same task/version would otherwise have satisfied the request. A
/// SQL-side `WHERE diff_content_hash_hex = ?` filter would make this
/// defense structurally unreachable — a malformed persisted value could
/// never text-match a well-formed queried hash in the first place, so
/// corruption in an unrelated row for the same task/version would silently
/// never surface.
fn load_diff_approval(
    connection: &Connection,
    task_id: TaskId,
    approved_task_version: u64,
    diff_content_hash: DiffContentHash,
) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
    let version = to_sql_integer(approved_task_version)
        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?;
    let mut statement = connection
        .prepare(
            "SELECT diff_content_hash_hex, approved_at_ms FROM task_diff_approvals
             WHERE task_id = ?1 AND approved_task_version = ?2",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map(params![task_id.to_string(), version], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(operation_failed)?;
    let mut found_at_ms = None;
    for row in rows {
        let (persisted_hash_hex, approved_at_ms) = row.map_err(operation_failed)?;
        let parsed_hash = DiffContentHash::from_hex(&persisted_hash_hex)
            .ok_or_else(|| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        if parsed_hash == diff_content_hash {
            found_at_ms = Some(approved_at_ms);
        }
    }
    Ok(found_at_ms.map(|approved_at_ms| DiffApprovalRecord {
        task_id,
        approved_task_version,
        diff_content_hash,
        approved_at_ms,
    }))
}

/// Shared core of `prepare_planning_context_package`/
/// `prepare_implementation_context_package`/`prepare_review_context_package`:
/// looks up the exact `(task_id, Claude, work_kind, expected_version,
/// ContextPackageV1)` consent and its FK-bound manifest together, and
/// either reuses both unchanged, inserts both fresh (consent first, so the
/// manifest's foreign key always has a row to bind to), or — if exactly one
/// of the pair exists — fails closed as `InvalidPersistenceState` without
/// writing anything. `work_kind` is fixed by the caller (one of the three
/// public methods above, each hardcoding its own), never taken from an
/// external caller of this private helper.
fn prepare_context_package(
    connection: &Connection,
    task_id: TaskId,
    work_kind: WorkKind,
    expected_version: u64,
    prepared_at_ms: i64,
) -> Result<ContextPackagePreparation, RepositoryError> {
    let existing_consent = load_provider_consent(
        connection,
        task_id,
        ProviderKind::Claude,
        work_kind,
        expected_version,
        ContextDataScope::ContextPackageV1,
    )?;
    let existing_manifest = load_context_package_manifest(
        connection,
        task_id,
        ProviderKind::Claude,
        work_kind,
        expected_version,
        ContextDataScope::ContextPackageV1,
    )?;
    match (existing_consent, existing_manifest) {
        (Some(consent), Some(manifest)) => Ok(ContextPackagePreparation { consent, manifest }),
        (None, None) => {
            let consent = ProviderConsent {
                task_id,
                provider: ProviderKind::Claude,
                work_kind,
                approved_task_version: expected_version,
                data_scope: ContextDataScope::ContextPackageV1,
                consented_at_ms: prepared_at_ms,
            };
            insert_provider_consent(connection, &consent)?;
            let manifest = ContextPackageManifestRecord {
                task_id,
                provider: ProviderKind::Claude,
                work_kind,
                approved_task_version: expected_version,
                data_scope: ContextDataScope::ContextPackageV1,
                created_at_ms: prepared_at_ms,
            };
            insert_context_package_manifest(connection, &manifest)?;
            Ok(ContextPackagePreparation { consent, manifest })
        }
        // Exactly one of the pair exists: an already-corrupted invariant
        // ("분리된 시점에 부분 저장" must never happen) that this method must
        // never silently repair by inserting the missing half under the
        // existing one.
        (Some(_), None) | (None, Some(_)) => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

/// Matches the `task_planning_results.plan_text` SQL `CHECK` bound in
/// `0007_task_planning_results.sql`. Enforced again here so a malformed
/// record is rejected before it ever reaches the database driver.
const MAX_PLAN_TEXT_LEN: usize = 100_000;

const fn planning_outcome_text(outcome: PlanningResultOutcome) -> &'static str {
    match outcome {
        PlanningResultOutcome::Completed => "Completed",
        PlanningResultOutcome::Failed => "Failed",
        PlanningResultOutcome::Cancelled => "Cancelled",
        PlanningResultOutcome::RecoveryRequired => "RecoveryRequired",
    }
}

fn validate_planning_result_shape(
    result: &TaskPlanningResultRecord,
) -> Result<(), RepositoryError> {
    if result.started_at_ms < 0 || result.completed_at_ms < result.started_at_ms {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    match (result.outcome, &result.plan_text) {
        (PlanningResultOutcome::Completed, Some(text)) => {
            if text.is_empty() || text.len() > MAX_PLAN_TEXT_LEN {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
        }
        (PlanningResultOutcome::Completed, None) => {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        (_, None) => {}
        (_, Some(_)) => return Err(repository_error(RepositoryErrorCode::InvalidAggregate)),
    }
    Ok(())
}

fn insert_planning_result(
    connection: &Connection,
    result: &TaskPlanningResultRecord,
) -> Result<(), RepositoryError> {
    let turn_count = result.turn_count.map(i64::from);
    connection
        .execute(
            "INSERT INTO task_planning_results (
                task_id, provider, work_kind, outcome, exit_code, turn_count,
                started_at_ms, completed_at_ms, plan_text
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                result.task_id.to_string(),
                provider_kind_text(result.provider),
                work_kind_text(result.work_kind),
                planning_outcome_text(result.outcome),
                result.exit_code,
                turn_count,
                result.started_at_ms,
                result.completed_at_ms,
                result.plan_text,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

fn load_planning_result(
    connection: &Connection,
    task_id: TaskId,
) -> Result<Option<TaskPlanningResultRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT provider, work_kind, outcome, exit_code, turn_count,
                    started_at_ms, completed_at_ms, plan_text
             FROM task_planning_results WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i32>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(operation_failed)?
        .map(
            |(
                provider,
                work_kind,
                outcome,
                exit_code,
                turn_count,
                started_at_ms,
                completed_at_ms,
                plan_text,
            )| {
                Ok(TaskPlanningResultRecord {
                    task_id,
                    provider: provider_kind_from_text(&provider)?,
                    work_kind: work_kind_from_text(&work_kind)?,
                    outcome: planning_outcome_from_text(&outcome)?,
                    exit_code,
                    turn_count: turn_count.map(u32::try_from).transpose().map_err(|_| {
                        repository_error(RepositoryErrorCode::InvalidPersistenceState)
                    })?,
                    started_at_ms,
                    completed_at_ms,
                    plan_text,
                })
            },
        )
        .transpose()
}

/// Matches the `task_review_results.review_text` SQL `CHECK` bound in
/// `0015_task_review_results.sql`. Enforced again here so a malformed
/// record is rejected before it ever reaches the database driver.
const MAX_REVIEW_TEXT_LEN: usize = 100_000;

const fn review_outcome_text(outcome: ReviewResultOutcome) -> &'static str {
    match outcome {
        ReviewResultOutcome::Completed => "Completed",
        ReviewResultOutcome::Failed => "Failed",
        ReviewResultOutcome::Cancelled => "Cancelled",
        ReviewResultOutcome::RecoveryRequired => "RecoveryRequired",
    }
}

fn review_outcome_from_text(value: &str) -> Result<ReviewResultOutcome, RepositoryError> {
    match value {
        "Completed" => Ok(ReviewResultOutcome::Completed),
        "Failed" => Ok(ReviewResultOutcome::Failed),
        "Cancelled" => Ok(ReviewResultOutcome::Cancelled),
        "RecoveryRequired" => Ok(ReviewResultOutcome::RecoveryRequired),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn validate_review_result_shape(result: &TaskReviewResultRecord) -> Result<(), RepositoryError> {
    if result.started_at_ms < 0 || result.completed_at_ms < result.started_at_ms {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    match (result.outcome, &result.review_text) {
        (ReviewResultOutcome::Completed, Some(text)) => {
            if text.is_empty() || text.len() > MAX_REVIEW_TEXT_LEN {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
        }
        (ReviewResultOutcome::Completed, None) => {
            return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
        }
        (_, None) => {}
        (_, Some(_)) => return Err(repository_error(RepositoryErrorCode::InvalidAggregate)),
    }
    Ok(())
}

fn insert_review_result(
    connection: &Connection,
    result: &TaskReviewResultRecord,
) -> Result<(), RepositoryError> {
    let turn_count = result.turn_count.map(i64::from);
    connection
        .execute(
            "INSERT INTO task_review_results (
                task_id, provider, work_kind, outcome, exit_code, turn_count,
                started_at_ms, completed_at_ms, review_text
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                result.task_id.to_string(),
                provider_kind_text(result.provider),
                work_kind_text(result.work_kind),
                review_outcome_text(result.outcome),
                result.exit_code,
                turn_count,
                result.started_at_ms,
                result.completed_at_ms,
                result.review_text,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

fn load_review_result(
    connection: &Connection,
    task_id: TaskId,
) -> Result<Option<TaskReviewResultRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT provider, work_kind, outcome, exit_code, turn_count,
                    started_at_ms, completed_at_ms, review_text
             FROM task_review_results WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i32>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(operation_failed)?
        .map(
            |(
                provider,
                work_kind,
                outcome,
                exit_code,
                turn_count,
                started_at_ms,
                completed_at_ms,
                review_text,
            )| {
                Ok(TaskReviewResultRecord {
                    task_id,
                    provider: provider_kind_from_text(&provider)?,
                    work_kind: work_kind_from_text(&work_kind)?,
                    outcome: review_outcome_from_text(&outcome)?,
                    exit_code,
                    turn_count: turn_count.map(u32::try_from).transpose().map_err(|_| {
                        repository_error(RepositoryErrorCode::InvalidPersistenceState)
                    })?,
                    started_at_ms,
                    completed_at_ms,
                    review_text,
                })
            },
        )
        .transpose()
}

const fn implementation_outcome_text(outcome: ImplementationResultOutcome) -> &'static str {
    match outcome {
        ImplementationResultOutcome::Completed => "Completed",
        ImplementationResultOutcome::Cancelled => "Cancelled",
        ImplementationResultOutcome::RecoveryRequired => "RecoveryRequired",
    }
}

fn implementation_outcome_from_text(
    value: &str,
) -> Result<ImplementationResultOutcome, RepositoryError> {
    match value {
        "Completed" => Ok(ImplementationResultOutcome::Completed),
        "Cancelled" => Ok(ImplementationResultOutcome::Cancelled),
        "RecoveryRequired" => Ok(ImplementationResultOutcome::RecoveryRequired),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn validate_implementation_result_shape(
    result: &TaskImplementationResultRecord,
) -> Result<(), RepositoryError> {
    if result.started_at_ms < 0 || result.completed_at_ms < result.started_at_ms {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn insert_implementation_result(
    connection: &Connection,
    result: &TaskImplementationResultRecord,
) -> Result<(), RepositoryError> {
    let turn_count = result.turn_count.map(i64::from);
    connection
        .execute(
            "INSERT INTO task_implementation_results (
                task_id, provider, work_kind, outcome, exit_code, turn_count,
                started_at_ms, completed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                result.task_id.to_string(),
                provider_kind_text(result.provider),
                work_kind_text(result.work_kind),
                implementation_outcome_text(result.outcome),
                result.exit_code,
                turn_count,
                result.started_at_ms,
                result.completed_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

fn load_implementation_result(
    connection: &Connection,
    task_id: TaskId,
) -> Result<Option<TaskImplementationResultRecord>, RepositoryError> {
    connection
        .query_row(
            "SELECT provider, work_kind, outcome, exit_code, turn_count,
                    started_at_ms, completed_at_ms
             FROM task_implementation_results WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i32>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()
        .map_err(operation_failed)?
        .map(
            |(
                provider,
                work_kind,
                outcome,
                exit_code,
                turn_count,
                started_at_ms,
                completed_at_ms,
            )| {
                Ok(TaskImplementationResultRecord {
                    task_id,
                    provider: provider_kind_from_text(&provider)?,
                    work_kind: work_kind_from_text(&work_kind)?,
                    outcome: implementation_outcome_from_text(&outcome)?,
                    exit_code,
                    turn_count: turn_count.map(u32::try_from).transpose().map_err(|_| {
                        repository_error(RepositoryErrorCode::InvalidPersistenceState)
                    })?,
                    started_at_ms,
                    completed_at_ms,
                })
            },
        )
        .transpose()
}

const fn validation_command_kind_text(kind: ValidationCommandKind) -> &'static str {
    match kind {
        ValidationCommandKind::Format => "Format",
        ValidationCommandKind::Lint => "Lint",
        ValidationCommandKind::Typecheck => "Typecheck",
        ValidationCommandKind::Test => "Test",
        ValidationCommandKind::Build => "Build",
    }
}

fn validation_command_kind_from_text(
    value: &str,
) -> Result<ValidationCommandKind, RepositoryError> {
    match value {
        "Format" => Ok(ValidationCommandKind::Format),
        "Lint" => Ok(ValidationCommandKind::Lint),
        "Typecheck" => Ok(ValidationCommandKind::Typecheck),
        "Test" => Ok(ValidationCommandKind::Test),
        "Build" => Ok(ValidationCommandKind::Build),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

const fn validation_execution_scope_text(scope: ValidationExecutionScope) -> &'static str {
    match scope {
        ValidationExecutionScope::TaskWorktree => "TaskWorktree",
        ValidationExecutionScope::ProjectRoot => "ProjectRoot",
    }
}

/// Matches the `task_validation_command_approvals.executable` SQL `CHECK`
/// bound in `0010_task_validation_command_approvals.sql`. Enforced again
/// here so a malformed record is rejected before it ever reaches the
/// database driver.
const MAX_VALIDATION_EXECUTABLE_LEN: usize = 256;

/// Matches the `approved_executable_path`/`tool_directory_path` SQL `CHECK`
/// bounds in `0011_validation_command_executable_binding.sql`.
const MAX_VALIDATION_PATH_LEN: usize = 4096;

/// A defense-in-depth check independent of the caller's candidate-membership
/// validation (`chatoms_application::validation_commands`, not yet wired at
/// this layer): rejects any value that is an absolute path or contains a
/// `..` path-traversal component, so an approval can never resolve outside
/// the task worktree it is scoped to.
fn is_worktree_confined(value: &str) -> bool {
    let path = std::path::Path::new(value);
    !path.is_absolute()
        && !path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
}

/// Defense-in-depth counterpart to [`is_worktree_confined`] for the fields
/// that must be an already-canonicalized absolute path (the approved
/// executable/tool-directory paths): requires an absolute path and rejects
/// any `..` component, so a caller bug can never persist a path-traversal
/// value even though the primary canonicalization happens one layer up, in
/// the `FilesystemIdentityPort` adapter the application layer calls before
/// building this record.
fn is_canonical_absolute_path(value: &str) -> bool {
    let path = std::path::Path::new(value);
    path.is_absolute()
        && !path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
}

/// Matches the hex-identity `CHECK` bounds in
/// `0011_validation_command_executable_binding.sql`, which mirror the
/// `root_volume_serial_hex`/`root_file_id_hex` convention from
/// `0002_git_isolation.sql`.
fn is_hex_of_length(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_validation_command_approval_shape(
    approval: &ValidationCommandApprovalRecord,
) -> Result<(), RepositoryError> {
    if approval.approved_at_ms < 0 {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if approval.executable.is_empty() || approval.executable.len() > MAX_VALIDATION_EXECUTABLE_LEN {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if !is_worktree_confined(&approval.executable)
        || approval
            .arguments
            .iter()
            .any(|argument| !is_worktree_confined(argument))
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if approval.approved_executable_path.is_empty()
        || approval.approved_executable_path.len() > MAX_VALIDATION_PATH_LEN
        || !is_canonical_absolute_path(&approval.approved_executable_path)
        || approval.tool_directory_path.is_empty()
        || approval.tool_directory_path.len() > MAX_VALIDATION_PATH_LEN
        || !is_canonical_absolute_path(&approval.tool_directory_path)
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if !is_hex_of_length(&approval.executable_volume_serial_hex, 16)
        || !is_hex_of_length(&approval.executable_file_id_hex, 32)
        || !is_hex_of_length(&approval.tool_directory_volume_serial_hex, 16)
        || !is_hex_of_length(&approval.tool_directory_file_id_hex, 32)
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    validate_optional_environment_binding(
        &approval.approved_cargo_home_path,
        &approval.cargo_home_volume_serial_hex,
        &approval.cargo_home_file_id_hex,
    )?;
    validate_optional_environment_binding(
        &approval.approved_rustup_home_path,
        &approval.rustup_home_volume_serial_hex,
        &approval.rustup_home_file_id_hex,
    )?;
    match approval.execution_scope {
        ValidationExecutionScope::TaskWorktree => {
            if approval.target_project_id.is_some()
                || approval.target_project_identity_revision.is_some()
                || approval.target_root_volume_serial_hex.is_some()
                || approval.target_root_file_id_hex.is_some()
            {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
        }
        ValidationExecutionScope::ProjectRoot => {
            let (Some(_), Some(_), Some(volume_serial_hex), Some(file_id_hex)) = (
                approval.target_project_id,
                approval.target_project_identity_revision,
                approval.target_root_volume_serial_hex.as_deref(),
                approval.target_root_file_id_hex.as_deref(),
            ) else {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            };
            if !is_hex_of_length(volume_serial_hex, 16) || !is_hex_of_length(file_id_hex, 32) {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
        }
    }
    Ok(())
}

/// Defense-in-depth shape check for an optional `CARGO_HOME`/`RUSTUP_HOME`
/// binding: the three fields must be all `None` (no approved override) or
/// all `Some` with a valid canonical absolute path and valid stable-identity
/// hex, mirroring the SQL `CHECK` in
/// `0012_validation_command_environment_binding.sql`.
fn validate_optional_environment_binding(
    path: &Option<String>,
    volume_serial_hex: &Option<String>,
    file_id_hex: &Option<String>,
) -> Result<(), RepositoryError> {
    match (path, volume_serial_hex, file_id_hex) {
        (None, None, None) => Ok(()),
        (Some(path), Some(volume_serial_hex), Some(file_id_hex)) => {
            if path.is_empty()
                || path.len() > MAX_VALIDATION_PATH_LEN
                || !is_canonical_absolute_path(path)
                || !is_hex_of_length(volume_serial_hex, 16)
                || !is_hex_of_length(file_id_hex, 32)
            {
                return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
            }
            Ok(())
        }
        _ => Err(repository_error(RepositoryErrorCode::InvalidAggregate)),
    }
}

fn insert_validation_command_approval(
    connection: &Connection,
    approval: &ValidationCommandApprovalRecord,
) -> Result<(), RepositoryError> {
    let arguments_json = serde_json::to_string(&approval.arguments)
        .map_err(|_| repository_error(RepositoryErrorCode::InvalidAggregate))?;
    connection
        .execute(
            "INSERT INTO task_validation_command_approvals (
                task_id, approved_task_version, execution_scope, command_kind, executable,
                arguments_json, approved_executable_path,
                executable_volume_serial_hex, executable_file_id_hex,
                tool_directory_path, tool_directory_volume_serial_hex,
                tool_directory_file_id_hex,
                approved_cargo_home_path, cargo_home_volume_serial_hex, cargo_home_file_id_hex,
                approved_rustup_home_path, rustup_home_volume_serial_hex, rustup_home_file_id_hex,
                target_project_id, target_project_identity_revision,
                target_root_volume_serial_hex, target_root_file_id_hex,
                approved_at_ms
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
             )",
            params![
                approval.task_id.to_string(),
                to_sql_integer(approval.approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                validation_execution_scope_text(approval.execution_scope),
                validation_command_kind_text(approval.kind),
                approval.executable,
                arguments_json,
                approval.approved_executable_path,
                approval.executable_volume_serial_hex,
                approval.executable_file_id_hex,
                approval.tool_directory_path,
                approval.tool_directory_volume_serial_hex,
                approval.tool_directory_file_id_hex,
                approval.approved_cargo_home_path,
                approval.cargo_home_volume_serial_hex,
                approval.cargo_home_file_id_hex,
                approval.approved_rustup_home_path,
                approval.rustup_home_volume_serial_hex,
                approval.rustup_home_file_id_hex,
                approval.target_project_id.map(|id| id.to_string()),
                approval
                    .target_project_identity_revision
                    .map(to_sql_integer)
                    .transpose()
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                approval.target_root_volume_serial_hex,
                approval.target_root_file_id_hex,
                approval.approved_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

fn load_validation_command_approvals(
    connection: &Connection,
    task_id: TaskId,
    approved_task_version: u64,
    execution_scope: ValidationExecutionScope,
) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
    let version = to_sql_integer(approved_task_version)
        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?;
    let mut statement = connection
        .prepare(
            "SELECT command_kind, executable, arguments_json, approved_executable_path,
                    executable_volume_serial_hex, executable_file_id_hex,
                    tool_directory_path, tool_directory_volume_serial_hex,
                    tool_directory_file_id_hex,
                    approved_cargo_home_path, cargo_home_volume_serial_hex,
                    cargo_home_file_id_hex, approved_rustup_home_path,
                    rustup_home_volume_serial_hex, rustup_home_file_id_hex,
                    target_project_id, target_project_identity_revision,
                    target_root_volume_serial_hex, target_root_file_id_hex, approved_at_ms
             FROM task_validation_command_approvals
             WHERE task_id = ?1 AND approved_task_version = ?2 AND execution_scope = ?3
             ORDER BY command_kind",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map(
            params![
                task_id.to_string(),
                version,
                validation_execution_scope_text(execution_scope)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, i64>(19)?,
                ))
            },
        )
        .map_err(operation_failed)?;
    let mut approvals = Vec::new();
    for row in rows {
        let (
            kind,
            executable,
            arguments_json,
            approved_executable_path,
            executable_volume_serial_hex,
            executable_file_id_hex,
            tool_directory_path,
            tool_directory_volume_serial_hex,
            tool_directory_file_id_hex,
            approved_cargo_home_path,
            cargo_home_volume_serial_hex,
            cargo_home_file_id_hex,
            approved_rustup_home_path,
            rustup_home_volume_serial_hex,
            rustup_home_file_id_hex,
            target_project_id,
            target_project_identity_revision,
            target_root_volume_serial_hex,
            target_root_file_id_hex,
            approved_at_ms,
        ) = row.map_err(operation_failed)?;
        let arguments: Vec<String> = serde_json::from_str(&arguments_json)
            .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        approvals.push(ValidationCommandApprovalRecord {
            task_id,
            approved_task_version,
            execution_scope,
            kind: validation_command_kind_from_text(&kind)?,
            executable,
            arguments,
            approved_executable_path,
            executable_volume_serial_hex,
            executable_file_id_hex,
            tool_directory_path,
            tool_directory_volume_serial_hex,
            tool_directory_file_id_hex,
            approved_cargo_home_path,
            cargo_home_volume_serial_hex,
            cargo_home_file_id_hex,
            approved_rustup_home_path,
            rustup_home_volume_serial_hex,
            rustup_home_file_id_hex,
            target_project_id: target_project_id
                .map(|value| ProjectId::from_str(&value).map_err(invalid_persistence))
                .transpose()?,
            target_project_identity_revision: target_project_identity_revision
                .map(|value| {
                    u64::try_from(value)
                        .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))
                })
                .transpose()?,
            target_root_volume_serial_hex,
            target_root_file_id_hex,
            approved_at_ms,
        });
    }
    Ok(approvals)
}

/// Matches the `safe_summary` SQL `CHECK` bound in
/// `0013_task_validation_command_results.sql`. Deliberately small: this is a
/// masked, already-bounded summary a future orchestration Unit produces,
/// never raw stdout/stderr.
const MAX_SAFE_SUMMARY_LEN: usize = 2000;

fn validate_validation_command_result_attempt_shape(
    attempt: &ValidationCommandResultAttempt,
) -> Result<(), RepositoryError> {
    if attempt.started_at_ms < 0 || attempt.completed_at_ms < attempt.started_at_ms {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    if attempt.safe_summary.is_empty() || attempt.safe_summary.len() > MAX_SAFE_SUMMARY_LEN {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    let exit_code_confirmed = matches!(
        attempt.outcome,
        ValidationCommandResultOutcome::Success | ValidationCommandResultOutcome::ExitFailure
    );
    if exit_code_confirmed != attempt.exit_code.is_some() {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn validation_command_approval_exists(
    connection: &Connection,
    task_id: TaskId,
    approved_task_version: u64,
    execution_scope: ValidationExecutionScope,
    kind: ValidationCommandKind,
) -> Result<bool, RepositoryError> {
    let version = to_sql_integer(approved_task_version)
        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?;
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM task_validation_command_approvals
                WHERE task_id = ?1 AND approved_task_version = ?2
                  AND execution_scope = ?3 AND command_kind = ?4
             )",
            params![
                task_id.to_string(),
                version,
                validation_execution_scope_text(execution_scope),
                validation_command_kind_text(kind)
            ],
            |row| row.get(0),
        )
        .map_err(operation_failed)
}

fn next_validation_command_result_sequence(
    connection: &Connection,
    task_id: TaskId,
    approved_task_version: u64,
    execution_scope: ValidationExecutionScope,
    kind: ValidationCommandKind,
) -> Result<u32, RepositoryError> {
    let version = to_sql_integer(approved_task_version)
        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?;
    let next_sequence: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(attempt_sequence), 0) + 1
             FROM task_validation_command_results
             WHERE task_id = ?1 AND approved_task_version = ?2
               AND execution_scope = ?3 AND command_kind = ?4",
            params![
                task_id.to_string(),
                version,
                validation_execution_scope_text(execution_scope),
                validation_command_kind_text(kind)
            ],
            |row| row.get(0),
        )
        .map_err(operation_failed)?;
    u32::try_from(next_sequence)
        .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))
}

fn insert_validation_command_result(
    connection: &Connection,
    attempt: &ValidationCommandResultAttempt,
    attempt_sequence: u32,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO task_validation_command_results (
                task_id, approved_task_version, execution_scope, command_kind, attempt_sequence,
                outcome, exit_code, safe_summary, started_at_ms, completed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                attempt.task_id.to_string(),
                to_sql_integer(attempt.approved_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                validation_execution_scope_text(attempt.execution_scope),
                validation_command_kind_text(attempt.kind),
                attempt_sequence,
                validation_command_result_outcome_text(attempt.outcome),
                attempt.exit_code,
                attempt.safe_summary,
                attempt.started_at_ms,
                attempt.completed_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

fn load_validation_command_results(
    connection: &Connection,
    task_id: TaskId,
    approved_task_version: u64,
    kind: ValidationCommandKind,
) -> Result<Vec<ValidationCommandResultRecord>, RepositoryError> {
    let version = to_sql_integer(approved_task_version)
        .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?;
    let mut statement = connection
        .prepare(
            "SELECT attempt_sequence, outcome, exit_code, safe_summary,
                    started_at_ms, completed_at_ms
             FROM task_validation_command_results
             WHERE task_id = ?1 AND approved_task_version = ?2
               AND execution_scope = 'TaskWorktree' AND command_kind = ?3
             ORDER BY attempt_sequence",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map(
            params![
                task_id.to_string(),
                version,
                validation_command_kind_text(kind)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .map_err(operation_failed)?;
    let mut results = Vec::new();
    for row in rows {
        let (attempt_sequence, outcome, exit_code, safe_summary, started_at_ms, completed_at_ms) =
            row.map_err(operation_failed)?;
        let attempt_sequence = u32::try_from(attempt_sequence)
            .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?;
        results.push(ValidationCommandResultRecord {
            task_id,
            approved_task_version,
            execution_scope: chatoms_domain::ValidationExecutionScope::TaskWorktree,
            kind,
            attempt_sequence,
            outcome: validation_command_result_outcome_from_text(&outcome)?,
            exit_code,
            safe_summary,
            started_at_ms,
            completed_at_ms,
        });
    }
    Ok(results)
}

const fn validation_command_result_outcome_text(
    outcome: ValidationCommandResultOutcome,
) -> &'static str {
    match outcome {
        ValidationCommandResultOutcome::Success => "Success",
        ValidationCommandResultOutcome::ExitFailure => "ExitFailure",
        ValidationCommandResultOutcome::TimedOut => "TimedOut",
        ValidationCommandResultOutcome::StdoutBoundExceeded => "StdoutBoundExceeded",
        ValidationCommandResultOutcome::Cancelled => "Cancelled",
        ValidationCommandResultOutcome::Uncertain => "Uncertain",
    }
}

fn validation_command_result_outcome_from_text(
    value: &str,
) -> Result<ValidationCommandResultOutcome, RepositoryError> {
    match value {
        "Success" => Ok(ValidationCommandResultOutcome::Success),
        "ExitFailure" => Ok(ValidationCommandResultOutcome::ExitFailure),
        "TimedOut" => Ok(ValidationCommandResultOutcome::TimedOut),
        "StdoutBoundExceeded" => Ok(ValidationCommandResultOutcome::StdoutBoundExceeded),
        "Cancelled" => Ok(ValidationCommandResultOutcome::Cancelled),
        "Uncertain" => Ok(ValidationCommandResultOutcome::Uncertain),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn validate_post_merge_validation_attempt_shape(
    attempt: &PostMergeValidationResultAttempt,
) -> Result<(), RepositoryError> {
    if attempt.execution_scope != ValidationExecutionScope::ProjectRoot
        || attempt.started_at_ms < 0
        || attempt.completed_at_ms < attempt.started_at_ms
        || attempt.safe_summary.is_empty()
        || attempt.safe_summary.len() > MAX_SAFE_SUMMARY_LEN
    {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    let exit_code_confirmed = matches!(
        attempt.outcome,
        PostMergeValidationResultOutcome::Success | PostMergeValidationResultOutcome::ExitFailure
    );
    if exit_code_confirmed != attempt.exit_code.is_some() {
        return Err(repository_error(RepositoryErrorCode::InvalidAggregate));
    }
    Ok(())
}

fn ensure_post_merge_approval(
    connection: &Connection,
    attempt: &PostMergeValidationResultAttempt,
) -> Result<(), RepositoryError> {
    if validation_command_approval_exists(
        connection,
        attempt.task_id,
        attempt.approval_task_version,
        ValidationExecutionScope::ProjectRoot,
        attempt.kind,
    )? {
        Ok(())
    } else {
        Err(repository_error(RepositoryErrorCode::InvalidAggregate))
    }
}

fn next_post_merge_validation_result_sequence(
    connection: &Connection,
    attempt: &PostMergeValidationResultAttempt,
) -> Result<u32, RepositoryError> {
    let next_sequence: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(attempt_sequence), 0) + 1
             FROM task_post_merge_validation_results
             WHERE task_id = ?1 AND approval_task_version = ?2
               AND post_merge_task_version = ?3 AND command_kind = ?4",
            params![
                attempt.task_id.to_string(),
                to_sql_integer(attempt.approval_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                to_sql_integer(attempt.post_merge_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                validation_command_kind_text(attempt.kind),
            ],
            |row| row.get(0),
        )
        .map_err(operation_failed)?;
    u32::try_from(next_sequence)
        .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))
}

fn insert_post_merge_validation_result(
    connection: &Connection,
    attempt: &PostMergeValidationResultAttempt,
    attempt_sequence: u32,
) -> Result<(), RepositoryError> {
    connection
        .execute(
            "INSERT INTO task_post_merge_validation_results (
                task_id, approval_task_version, post_merge_task_version,
                execution_scope, command_kind, attempt_sequence, outcome,
                exit_code, safe_summary, started_at_ms, completed_at_ms
             ) VALUES (?1, ?2, ?3, 'ProjectRoot', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                attempt.task_id.to_string(),
                to_sql_integer(attempt.approval_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                to_sql_integer(attempt.post_merge_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                validation_command_kind_text(attempt.kind),
                attempt_sequence,
                post_merge_validation_outcome_text(attempt.outcome),
                attempt.exit_code,
                attempt.safe_summary,
                attempt.started_at_ms,
                attempt.completed_at_ms,
            ],
        )
        .map_err(|source| {
            RepositoryError::with_source(RepositoryErrorCode::InvalidAggregate, source)
        })?;
    Ok(())
}

fn load_post_merge_validation_results(
    connection: &Connection,
    task_id: TaskId,
    approval_task_version: u64,
    post_merge_task_version: u64,
    kind: ValidationCommandKind,
) -> Result<Vec<PostMergeValidationResultRecord>, RepositoryError> {
    let mut statement = connection
        .prepare(
            "SELECT attempt_sequence, outcome, exit_code, safe_summary,
                    started_at_ms, completed_at_ms
             FROM task_post_merge_validation_results
             WHERE task_id = ?1 AND approval_task_version = ?2
               AND post_merge_task_version = ?3 AND command_kind = ?4
             ORDER BY attempt_sequence",
        )
        .map_err(operation_failed)?;
    let rows = statement
        .query_map(
            params![
                task_id.to_string(),
                to_sql_integer(approval_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                to_sql_integer(post_merge_task_version)
                    .map_err(|_| repository_error(RepositoryErrorCode::VersionConflict))?,
                validation_command_kind_text(kind),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .map_err(operation_failed)?;
    let mut results = Vec::new();
    for row in rows {
        let (sequence, outcome, exit_code, safe_summary, started_at_ms, completed_at_ms) =
            row.map_err(operation_failed)?;
        results.push(PostMergeValidationResultRecord {
            task_id,
            approval_task_version,
            post_merge_task_version,
            execution_scope: ValidationExecutionScope::ProjectRoot,
            kind,
            attempt_sequence: u32::try_from(sequence)
                .map_err(|_| repository_error(RepositoryErrorCode::InvalidPersistenceState))?,
            outcome: post_merge_validation_outcome_from_text(&outcome)?,
            exit_code,
            safe_summary,
            started_at_ms,
            completed_at_ms,
        });
    }
    Ok(results)
}

fn post_merge_validation_record(
    attempt: &PostMergeValidationResultAttempt,
    attempt_sequence: u32,
) -> PostMergeValidationResultRecord {
    PostMergeValidationResultRecord {
        task_id: attempt.task_id,
        approval_task_version: attempt.approval_task_version,
        post_merge_task_version: attempt.post_merge_task_version,
        execution_scope: attempt.execution_scope,
        kind: attempt.kind,
        attempt_sequence,
        outcome: attempt.outcome,
        exit_code: attempt.exit_code,
        safe_summary: attempt.safe_summary.clone(),
        started_at_ms: attempt.started_at_ms,
        completed_at_ms: attempt.completed_at_ms,
    }
}

const fn post_merge_validation_outcome_text(
    outcome: PostMergeValidationResultOutcome,
) -> &'static str {
    match outcome {
        PostMergeValidationResultOutcome::Success => "Success",
        PostMergeValidationResultOutcome::ExitFailure => "ExitFailure",
        PostMergeValidationResultOutcome::TimedOut => "TimedOut",
        PostMergeValidationResultOutcome::StdoutBoundExceeded => "StdoutBoundExceeded",
        PostMergeValidationResultOutcome::BindingRejected => "BindingRejected",
        PostMergeValidationResultOutcome::Cancelled => "Cancelled",
        PostMergeValidationResultOutcome::Uncertain => "Uncertain",
    }
}

fn post_merge_validation_outcome_from_text(
    value: &str,
) -> Result<PostMergeValidationResultOutcome, RepositoryError> {
    match value {
        "Success" => Ok(PostMergeValidationResultOutcome::Success),
        "ExitFailure" => Ok(PostMergeValidationResultOutcome::ExitFailure),
        "TimedOut" => Ok(PostMergeValidationResultOutcome::TimedOut),
        "StdoutBoundExceeded" => Ok(PostMergeValidationResultOutcome::StdoutBoundExceeded),
        "BindingRejected" => Ok(PostMergeValidationResultOutcome::BindingRejected),
        "Cancelled" => Ok(PostMergeValidationResultOutcome::Cancelled),
        "Uncertain" => Ok(PostMergeValidationResultOutcome::Uncertain),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn provider_kind_from_text(value: &str) -> Result<ProviderKind, RepositoryError> {
    match value {
        "Claude" => Ok(ProviderKind::Claude),
        "Codex" => Ok(ProviderKind::Codex),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn work_kind_from_text(value: &str) -> Result<WorkKind, RepositoryError> {
    match value {
        "Planning" => Ok(WorkKind::Planning),
        "Implementation" => Ok(WorkKind::Implementation),
        "Review" => Ok(WorkKind::Review),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
    }
}

fn planning_outcome_from_text(value: &str) -> Result<PlanningResultOutcome, RepositoryError> {
    match value {
        "Completed" => Ok(PlanningResultOutcome::Completed),
        "Failed" => Ok(PlanningResultOutcome::Failed),
        "Cancelled" => Ok(PlanningResultOutcome::Cancelled),
        "RecoveryRequired" => Ok(PlanningResultOutcome::RecoveryRequired),
        _ => Err(repository_error(
            RepositoryErrorCode::InvalidPersistenceState,
        )),
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
        TaskState::Planning => "Planning",
        TaskState::AwaitingDesignApproval => "AwaitingDesignApproval",
        TaskState::Implementing => "Implementing",
        TaskState::Testing => "Testing",
        TaskState::AutoFixing => "AutoFixing",
        TaskState::Reviewing => "Reviewing",
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
        "Planning" => Ok(TaskState::Planning),
        "AwaitingDesignApproval" => Ok(TaskState::AwaitingDesignApproval),
        "Implementing" => Ok(TaskState::Implementing),
        "Testing" => Ok(TaskState::Testing),
        "AutoFixing" => Ok(TaskState::AutoFixing),
        "Reviewing" => Ok(TaskState::Reviewing),
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
