#![allow(dead_code)]

use std::{path::Path, time::Duration};

use chatoms_infrastructure::database::{DatabaseConnection, MigrationOutcome, MigrationRunner};
use rusqlite::{Connection, Error, ErrorCode, Transaction, params};

pub struct TestDatabase {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
}

impl TestDatabase {
    pub fn empty() -> Self {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("chatoms.sqlite3");
        Self {
            _directory: directory,
            path,
        }
    }

    pub fn migrated() -> Self {
        let database = Self::empty();
        database.migrate();
        database
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn migrate(&self) -> MigrationOutcome {
        let mut connection = DatabaseConnection::open(&self.path).expect("open database");
        MigrationRunner::default()
            .run(&mut connection)
            .expect("run production migrations")
    }

    pub fn open_raw(&self) -> Connection {
        open_configured(&self.path)
    }
}

pub fn open_configured(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open test database");
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .expect("set busy timeout");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;",
        )
        .expect("configure test connection");
    connection
}

pub fn insert_project(connection: &Connection, id: &str) {
    connection
        .execute(
            "INSERT INTO projects (id, name, root_path, created_at_ms, updated_at_ms)
             VALUES (?1, 'Project', 'C:/project', 100, 100)",
            [id],
        )
        .expect("insert project");
}

pub fn insert_task(
    transaction: &Transaction<'_>,
    id: &str,
    project_id: &str,
    state: &str,
    version: i64,
    resume_target: Option<&str>,
    terminal_at_ms: Option<i64>,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO tasks (
            id, project_id, state, version, task_branch_identity,
            resume_target_state, created_at_ms, updated_at_ms, terminal_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 100, 100, ?7)",
        params![
            id,
            project_id,
            state,
            version,
            format!("ai-task/{id}"),
            resume_target,
            terminal_at_ms
        ],
    )?;
    Ok(())
}

pub fn insert_initial_transition(
    transaction: &Transaction<'_>,
    task_id: &str,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO task_state_transitions (
            id, task_id, sequence, from_state, to_state, task_version,
            actor_kind, reason_code, occurred_at_ms
         ) VALUES (?1, ?2, 1, NULL, 'Created', 0, 'application', 'task.created', 100)",
        params![format!("transition-{task_id}-1"), task_id],
    )?;
    Ok(())
}

pub fn insert_lease(transaction: &Transaction<'_>, task_id: &str) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO active_task_leases (singleton_key, task_id, acquired_at_ms)
         VALUES (1, ?1, 100)",
        [task_id],
    )?;
    Ok(())
}

pub fn create_active_task(connection: &mut Connection, task_id: &str, project_id: &str) {
    let transaction = connection.transaction().expect("begin task transaction");
    insert_task(&transaction, task_id, project_id, "Created", 0, None, None).expect("insert task");
    insert_initial_transition(&transaction, task_id).expect("insert transition");
    insert_lease(&transaction, task_id).expect("insert lease");
    transaction.commit().expect("commit active task");
}

pub fn count_rows(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
}

pub fn table_exists(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1
             )",
            [table],
            |row| row.get(0),
        )
        .expect("query table existence")
}

pub fn foreign_key_violation_count(connection: &Connection) -> usize {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .expect("prepare foreign_key_check");
    let mut rows = statement.query([]).expect("run foreign_key_check");
    let mut count = 0;
    while rows.next().expect("read foreign_key_check").is_some() {
        count += 1;
    }
    count
}

pub fn is_constraint_error(error: &Error) -> bool {
    matches!(
        error,
        Error::SqliteFailure(details, _) if details.code == ErrorCode::ConstraintViolation
    )
}
