use std::{
    path::{Path, PathBuf},
    sync::{Arc, Barrier, mpsc},
    thread,
    time::Duration,
};

use rusqlite::{Connection, Error, ErrorCode, Transaction, TransactionBehavior, params};
use tempfile::TempDir;

const ALL_STATES: &[&str] = &[
    "Created",
    "ProjectValidated",
    "AwaitingGitInitApproval",
    "GitInitialized",
    "WorktreeCreating",
    "WorktreeReady",
    "Planning",
    "AwaitingDesignApproval",
    "Implementing",
    "Testing",
    "AutoFixing",
    "Reviewing",
    "ReviewFixing",
    "AwaitingUserDiffApproval",
    "Merging",
    "MergeConflict",
    "PostMergeTesting",
    "Completed",
    "Paused",
    "Failed",
    "RecoveryRequired",
    "UnknownExternalEffect",
    "Cancelled",
    "CleanupPending",
    "Archived",
];

const LEASE_FREE_STATES: &[&str] = &[
    "Completed",
    "Failed",
    "Cancelled",
    "CleanupPending",
    "Archived",
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

struct TestDatabase {
    _directory: TempDir,
    path: PathBuf,
}

impl TestDatabase {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("chatoms-feasibility.sqlite3");
        let connection = open_configured(&path);
        connection
            .execute_batch(&schema_sql())
            .expect("minimal feasibility schema");
        drop(connection);

        Self {
            _directory: directory,
            path,
        }
    }

    fn open(&self) -> Connection {
        open_configured(&self.path)
    }
}

fn quoted_states(states: &[&str]) -> String {
    states
        .iter()
        .map(|state| format!("'{state}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn schema_sql() -> String {
    let all_states = quoted_states(ALL_STATES);
    let lease_free_states = quoted_states(LEASE_FREE_STATES);

    format!(
        r#"
        CREATE TABLE tasks (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL CHECK (length(project_id) > 0),
            task_branch_identity TEXT NOT NULL UNIQUE
                CHECK (length(task_branch_identity) > 0),
            state TEXT NOT NULL CHECK (state IN ({all_states})),
            version INTEGER NOT NULL CHECK (version >= 1),
            lease_required_key INTEGER GENERATED ALWAYS AS (
                CASE WHEN state IN ({lease_free_states}) THEN NULL ELSE 1 END
            ) STORED,
            UNIQUE (id, lease_required_key),
            FOREIGN KEY (id, lease_required_key)
                REFERENCES active_task_leases (task_id, singleton_key)
                DEFERRABLE INITIALLY DEFERRED
        );

        CREATE TABLE active_task_leases (
            singleton_key INTEGER PRIMARY KEY CHECK (singleton_key = 1),
            task_id TEXT NOT NULL UNIQUE,
            acquired_at_ms INTEGER NOT NULL,
            UNIQUE (task_id, singleton_key),
            FOREIGN KEY (task_id, singleton_key)
                REFERENCES tasks (id, lease_required_key)
                DEFERRABLE INITIALLY DEFERRED
        );

        CREATE TABLE task_state_transitions (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            from_state TEXT CHECK (from_state IS NULL OR from_state IN ({all_states})),
            to_state TEXT NOT NULL CHECK (to_state IN ({all_states})),
            task_version INTEGER NOT NULL CHECK (task_version >= 1),
            actor TEXT NOT NULL CHECK (length(actor) > 0),
            reason TEXT NOT NULL,
            occurred_at_ms INTEGER NOT NULL,
            UNIQUE (task_id, task_version),
            FOREIGN KEY (task_id)
                REFERENCES tasks (id)
                DEFERRABLE INITIALLY DEFERRED
        );

        CREATE TRIGGER tasks_project_id_immutable
        BEFORE UPDATE OF project_id ON tasks
        FOR EACH ROW
        WHEN NEW.project_id IS NOT OLD.project_id
        BEGIN
            SELECT RAISE(ABORT, 'tasks.project_id is immutable');
        END;

        CREATE TRIGGER tasks_branch_identity_immutable
        BEFORE UPDATE OF task_branch_identity ON tasks
        FOR EACH ROW
        WHEN NEW.task_branch_identity IS NOT OLD.task_branch_identity
        BEGIN
            SELECT RAISE(ABORT, 'tasks.task_branch_identity is immutable');
        END;

        CREATE TRIGGER active_lease_nonterminal_delete_guard
        BEFORE DELETE ON active_task_leases
        FOR EACH ROW
        WHEN EXISTS (
            SELECT 1
            FROM tasks
            WHERE id = OLD.task_id AND lease_required_key = 1
        )
        BEGIN
            SELECT RAISE(ABORT, 'nonterminal task lease cannot be deleted');
        END;
        "#
    )
}

fn open_configured(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open file-backed SQLite database");
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .expect("set SQLite busy timeout");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;",
        )
        .expect("apply connection PRAGMAs");
    connection
}

fn insert_task(
    transaction: &Transaction<'_>,
    task_id: &str,
    state: &str,
    version: i64,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO tasks (
            id, project_id, task_branch_identity, state, version
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            task_id,
            format!("project-{task_id}"),
            format!("branch-{task_id}"),
            state,
            version
        ],
    )?;
    Ok(())
}

fn insert_transition(
    transaction: &Transaction<'_>,
    transition_id: &str,
    task_id: &str,
    from_state: Option<&str>,
    to_state: &str,
    task_version: i64,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO task_state_transitions (
            id, task_id, from_state, to_state, task_version, actor, reason, occurred_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'test', 'feasibility', ?6)",
        params![
            transition_id,
            task_id,
            from_state,
            to_state,
            task_version,
            task_version * 1_000
        ],
    )?;
    Ok(())
}

fn insert_lease(transaction: &Transaction<'_>, task_id: &str) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO active_task_leases (singleton_key, task_id, acquired_at_ms)
         VALUES (1, ?1, 1_000)",
        [task_id],
    )?;
    Ok(())
}

fn create_active_task(connection: &mut Connection, task_id: &str) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    insert_task(&transaction, task_id, "Created", 1)?;
    insert_transition(
        &transaction,
        &format!("transition-{task_id}-1"),
        task_id,
        None,
        "Created",
        1,
    )?;
    insert_lease(&transaction, task_id)?;
    transaction.commit()
}

fn count_rows(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count table rows")
}

fn foreign_key_violation_count(connection: &Connection) -> usize {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .expect("prepare foreign_key_check");
    let mut rows = statement.query([]).expect("run foreign_key_check");
    let mut count = 0;
    while rows.next().expect("read foreign_key_check row").is_some() {
        count += 1;
    }
    count
}

fn is_constraint_error(error: &Error) -> bool {
    matches!(
        error,
        Error::SqliteFailure(details, _) if details.code == ErrorCode::ConstraintViolation
    )
}

fn assert_constraint_error(error: &Error) {
    assert!(
        is_constraint_error(error),
        "expected SQLite constraint failure, got {error:?}"
    );
}

#[test]
fn bundled_sqlite_supports_required_features() {
    let version = rusqlite::version();
    let version_number = rusqlite::version_number();
    println!("bundled SQLite version: {version} ({version_number})");
    println!("documented states: {}", ALL_STATES.join(", "));

    assert!(
        version_number >= 3_031_000,
        "generated columns require SQLite 3.31.0 or newer"
    );
    assert_eq!(ALL_STATES.len(), 25);
    assert_eq!(LEASE_FREE_STATES.len(), 5);
}

#[test]
fn configured_connection_verifies_required_pragmas() {
    let database = TestDatabase::new();
    let connection = database.open();

    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("read foreign_keys");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("read journal_mode");
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .expect("read synchronous");
    let busy_timeout: i64 = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("read busy_timeout");

    println!(
        "PRAGMA foreign_keys={foreign_keys}, journal_mode={journal_mode}, synchronous={synchronous}, busy_timeout={busy_timeout}"
    );
    assert_eq!(foreign_keys, 1);
    assert_eq!(journal_mode, "wal");
    assert_eq!(synchronous, 2);
    assert_eq!(busy_timeout, BUSY_TIMEOUT.as_millis() as i64);
}

#[test]
fn schema_passes_foreign_key_check() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-valid").expect("create valid active task");

    assert_eq!(foreign_key_violation_count(&connection), 0);
}

#[test]
fn task_without_required_lease_fails_at_commit() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    let transaction = connection.transaction().expect("begin transaction");
    insert_task(&transaction, "task-without-lease", "Created", 1).expect("insert task");

    let error = transaction.commit().expect_err("commit must require lease");
    assert_constraint_error(&error);
    assert_eq!(count_rows(&connection, "tasks"), 0);
}

#[test]
fn task_transition_and_lease_commit_together() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-created").expect("atomic task creation");

    assert_eq!(count_rows(&connection, "tasks"), 1);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 1);
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

#[test]
fn failed_lifecycle_step_rolls_back_entire_create_transaction() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    let transaction = connection.transaction().expect("begin transaction");
    insert_task(&transaction, "task-rollback", "Created", 1).expect("insert task");
    let error = insert_transition(
        &transaction,
        "transition-invalid",
        "task-rollback",
        None,
        "InvalidState",
        1,
    )
    .expect_err("invalid transition must fail");
    assert_constraint_error(&error);
    transaction.rollback().expect("rollback failed creation");

    assert_eq!(count_rows(&connection, "tasks"), 0);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 0);
    assert_eq!(count_rows(&connection, "active_task_leases"), 0);
}

#[test]
fn second_nonterminal_task_conflicts_with_singleton_lease() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-first").expect("create first task");

    let transaction = connection.transaction().expect("begin second creation");
    insert_task(&transaction, "task-second", "Created", 1).expect("insert second task");
    insert_transition(
        &transaction,
        "transition-task-second-1",
        "task-second",
        None,
        "Created",
        1,
    )
    .expect("insert second transition");
    let error = insert_lease(&transaction, "task-second").expect_err("singleton must conflict");
    assert_constraint_error(&error);
    transaction.rollback().expect("rollback second task");

    assert_eq!(count_rows(&connection, "tasks"), 1);
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
}

#[test]
fn terminal_update_without_lease_delete_fails_at_commit() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-terminal-missing-delete")
        .expect("create active task");

    let transaction = connection.transaction().expect("begin terminal transition");
    transaction
        .execute(
            "UPDATE tasks SET state = 'Completed', version = 2 WHERE id = ?1",
            ["task-terminal-missing-delete"],
        )
        .expect("update task state");
    insert_transition(
        &transaction,
        "transition-terminal-missing-delete",
        "task-terminal-missing-delete",
        Some("Created"),
        "Completed",
        2,
    )
    .expect("insert terminal transition");

    let error = transaction
        .commit()
        .expect_err("terminal commit must delete lease");
    assert_constraint_error(&error);
    let state: String = connection
        .query_row(
            "SELECT state FROM tasks WHERE id = ?1",
            ["task-terminal-missing-delete"],
            |row| row.get(0),
        )
        .expect("read rolled back state");
    assert_eq!(state, "Created");
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
}

#[test]
fn terminal_transition_and_lease_delete_commit_together() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-terminal").expect("create active task");

    let transaction = connection.transaction().expect("begin terminal transition");
    transaction
        .execute(
            "UPDATE tasks SET state = 'Completed', version = 2 WHERE id = ?1",
            ["task-terminal"],
        )
        .expect("update terminal state");
    insert_transition(
        &transaction,
        "transition-task-terminal-2",
        "task-terminal",
        Some("Created"),
        "Completed",
        2,
    )
    .expect("insert terminal transition");
    transaction
        .execute(
            "DELETE FROM active_task_leases WHERE task_id = ?1",
            ["task-terminal"],
        )
        .expect("delete terminal lease");
    transaction.commit().expect("commit terminal transition");

    assert_eq!(count_rows(&connection, "active_task_leases"), 0);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 2);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

#[test]
fn deleting_nonterminal_lease_before_state_change_is_rejected() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-delete-first").expect("create active task");

    let transaction = connection.transaction().expect("begin transaction");
    let error = transaction
        .execute(
            "DELETE FROM active_task_leases WHERE task_id = ?1",
            ["task-delete-first"],
        )
        .expect_err("nonterminal lease delete must fail immediately");
    assert_constraint_error(&error);
    transaction.rollback().expect("rollback rejected delete");
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
}

#[test]
fn terminal_task_cannot_hold_lease() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    let transaction = connection
        .transaction()
        .expect("begin terminal task insert");
    insert_task(&transaction, "task-already-terminal", "Completed", 1)
        .expect("insert terminal task");
    insert_transition(
        &transaction,
        "transition-task-already-terminal-1",
        "task-already-terminal",
        None,
        "Completed",
        1,
    )
    .expect("insert terminal history");
    transaction.commit().expect("commit terminal task");

    let transaction = connection
        .transaction()
        .expect("begin illegal lease insert");
    insert_lease(&transaction, "task-already-terminal").expect("deferred lease insert");
    let error = transaction
        .commit()
        .expect_err("terminal task lease must fail at commit");
    assert_constraint_error(&error);
    assert_eq!(count_rows(&connection, "active_task_leases"), 0);
}

#[test]
fn lease_cannot_reference_missing_task() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    let transaction = connection.transaction().expect("begin orphan lease insert");
    insert_lease(&transaction, "task-missing").expect("deferred orphan lease insert");
    let error = transaction
        .commit()
        .expect_err("orphan lease must fail at commit");
    assert_constraint_error(&error);
    assert_eq!(count_rows(&connection, "active_task_leases"), 0);
}

#[test]
fn invalid_state_strings_are_rejected() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    let transaction = connection.transaction().expect("begin invalid task insert");
    let error = insert_task(&transaction, "task-invalid-state", "InvalidState", 1)
        .expect_err("invalid task state must fail");
    assert_constraint_error(&error);
    transaction.rollback().expect("rollback invalid task");

    create_active_task(&mut connection, "task-valid-state").expect("create valid task");
    let transaction = connection
        .transaction()
        .expect("begin invalid transition insert");
    let error = insert_transition(
        &transaction,
        "transition-invalid-state",
        "task-valid-state",
        Some("Created"),
        "InvalidState",
        2,
    )
    .expect_err("invalid transition state must fail");
    assert_constraint_error(&error);
    transaction.rollback().expect("rollback invalid transition");
}

#[test]
fn project_id_is_immutable() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-project-immutable").expect("create active task");

    let error = connection
        .execute(
            "UPDATE tasks SET project_id = 'other-project' WHERE id = ?1",
            ["task-project-immutable"],
        )
        .expect_err("project_id update must fail");
    assert_constraint_error(&error);
}

#[test]
fn branch_identity_is_immutable() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-branch-immutable").expect("create active task");

    let error = connection
        .execute(
            "UPDATE tasks SET task_branch_identity = 'other-branch' WHERE id = ?1",
            ["task-branch-immutable"],
        )
        .expect_err("branch identity update must fail");
    assert_constraint_error(&error);
}

#[test]
fn transition_insert_failure_rolls_back_state_and_lease_delete() {
    let database = TestDatabase::new();
    let mut connection = database.open();
    create_active_task(&mut connection, "task-transition-rollback").expect("create active task");

    let transaction = connection.transaction().expect("begin terminal transition");
    transaction
        .execute(
            "UPDATE tasks SET state = 'Completed', version = 2 WHERE id = ?1",
            ["task-transition-rollback"],
        )
        .expect("update task state");
    insert_transition(
        &transaction,
        "transition-task-transition-rollback-2",
        "task-transition-rollback",
        Some("Created"),
        "Completed",
        2,
    )
    .expect("insert terminal transition");
    transaction
        .execute(
            "DELETE FROM active_task_leases WHERE task_id = ?1",
            ["task-transition-rollback"],
        )
        .expect("delete terminal lease");

    // Inject a later transition persistence failure to prove rollback restores
    // both the state update and the already-deleted lease.
    let error = insert_transition(
        &transaction,
        "transition-task-transition-rollback-duplicate",
        "task-transition-rollback",
        Some("Created"),
        "Completed",
        2,
    )
    .expect_err("duplicate task version must fail");
    assert_constraint_error(&error);
    transaction
        .rollback()
        .expect("rollback failed transition transaction");

    let (state, version): (String, i64) = connection
        .query_row(
            "SELECT state, version FROM tasks WHERE id = ?1",
            ["task-transition-rollback"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read restored task");
    assert_eq!((state.as_str(), version), ("Created", 1));
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 1);
}

#[test]
fn two_connections_contend_for_singleton_lease_exactly_one_wins() {
    let database = TestDatabase::new();
    let path = Arc::new(database.path.clone());
    let barrier = Arc::new(Barrier::new(2));
    let (result_sender, result_receiver) = mpsc::channel();
    let mut handles = Vec::new();

    for task_id in ["task-concurrent-a", "task-concurrent-b"] {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        let result_sender = result_sender.clone();
        handles.push(thread::spawn(move || {
            let mut connection = open_configured(path.as_ref());
            barrier.wait();
            let result = (|| -> rusqlite::Result<()> {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                insert_task(&transaction, task_id, "Created", 1)?;
                insert_transition(
                    &transaction,
                    &format!("transition-{task_id}-1"),
                    task_id,
                    None,
                    "Created",
                    1,
                )?;
                insert_lease(&transaction, task_id)?;
                transaction.commit()
            })();
            result_sender.send(result).expect("send thread result");
        }));
    }
    drop(result_sender);

    let results = (0..2)
        .map(|_| {
            result_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrency test timed out")
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("concurrency thread panicked");
    }

    let success_count = results.iter().filter(|result| result.is_ok()).count();
    let constraint_count = results
        .iter()
        .filter(|result| result.as_ref().is_err_and(is_constraint_error))
        .count();
    println!(
        "concurrent lease outcomes: successes={success_count}, constraints={constraint_count}"
    );
    assert_eq!(success_count, 1);
    assert_eq!(constraint_count, 1);

    let connection = database.open();
    assert_eq!(count_rows(&connection, "tasks"), 1);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 1);
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

#[test]
fn foreign_keys_off_connection_can_bypass_deferred_fk_defense() {
    let database = TestDatabase::new();

    // This is a threat-boundary test, not production policy. SQLite schema alone
    // cannot defend against an external or misconfigured connection that turns
    // per-connection foreign-key enforcement off. Production must set it to ON
    // and verify the result on every connection.
    let connection = Connection::open(&database.path).expect("open unconfigured connection");
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .expect("set busy timeout");
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("explicitly disable foreign keys before any transaction");
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("read disabled foreign_keys");
    assert_eq!(foreign_keys, 0);

    connection
        .execute(
            "INSERT INTO active_task_leases (singleton_key, task_id, acquired_at_ms)
             VALUES (1, 'task-does-not-exist', 1_000)",
            [],
        )
        .expect("foreign_keys=OFF bypasses orphan defense");
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 1);
}
