use std::str::FromStr;

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId, TaskSnapshot, TaskState,
    TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot,
};
use chatoms_ports::repository::{
    ActiveLease, FoundationRepository, ProjectSummary, RepositoryError, RepositoryErrorCode,
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
            "SELECT id, name, root_path, created_at_ms, updated_at_ms
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
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(operation_failed)?;
    let mut projects = Vec::new();
    for row in rows {
        let (id, name, root_path, created_at_ms, updated_at_ms) = row.map_err(operation_failed)?;
        if name.is_empty()
            || root_path.is_empty()
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
            created_at_ms,
            updated_at_ms,
        });
    }
    Ok(projects)
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
