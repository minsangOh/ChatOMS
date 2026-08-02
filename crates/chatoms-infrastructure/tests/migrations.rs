mod support;

use chatoms_infrastructure::database::{
    DatabaseConnection, DatabaseError, FOUNDATION_MIGRATION, Migration, MigrationRunner,
    checksum_sha256, validate_registry,
};
use rusqlite::{Connection, params};

use support::{
    TestDatabase, count_rows, create_active_task, foreign_key_violation_count, insert_lease,
    insert_project, insert_task, is_constraint_error, table_exists,
};

fn run_registry(
    database: &TestDatabase,
    registry: &'static [Migration],
) -> Result<chatoms_infrastructure::database::MigrationOutcome, DatabaseError> {
    let mut connection = DatabaseConnection::open(database.path()).expect("open database");
    MigrationRunner::new(registry).run(&mut connection)
}

#[test]
fn production_registry_and_checksum_policy_are_valid() {
    assert_eq!(FOUNDATION_MIGRATION.len(), 1);
    assert_eq!(FOUNDATION_MIGRATION[0].version, 1);
    assert_eq!(FOUNDATION_MIGRATION[0].name, "foundation");
    validate_registry(&FOUNDATION_MIGRATION).expect("production registry must be valid");

    let checksum = FOUNDATION_MIGRATION[0].checksum_sha256();
    assert_eq!(checksum.len(), 64);
    assert!(
        checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(
        checksum_sha256(FOUNDATION_MIGRATION[0].sql.as_bytes()),
        checksum
    );
    assert_ne!(checksum_sha256(b"line\n"), checksum_sha256(b"line\r\n"));
    assert_eq!(
        checksum_sha256(b"same bytes"),
        checksum_sha256(b"same bytes")
    );
}

#[test]
fn registry_rejects_zero_non_one_start_duplicates_order_and_empty_fields() {
    static ZERO: [Migration; 1] = [Migration::new(0, "zero", "SELECT 1;")];
    static START_TWO: [Migration; 1] = [Migration::new(2, "two", "SELECT 1;")];
    static DUPLICATE: [Migration; 2] = [
        Migration::new(1, "one", "SELECT 1;"),
        Migration::new(1, "duplicate", "SELECT 2;"),
    ];
    static REVERSED: [Migration; 3] = [
        Migration::new(1, "one", "SELECT 1;"),
        Migration::new(3, "three", "SELECT 3;"),
        Migration::new(2, "two", "SELECT 2;"),
    ];
    static EMPTY_NAME: [Migration; 1] = [Migration::new(1, "", "SELECT 1;")];
    static EMPTY_SQL: [Migration; 1] = [Migration::new(1, "empty", "")];

    for registry in [
        &[][..],
        &ZERO,
        &START_TWO,
        &DUPLICATE,
        &REVERSED,
        &EMPTY_NAME,
        &EMPTY_SQL,
    ] {
        assert!(matches!(
            validate_registry(registry),
            Err(DatabaseError::MigrationRegistryInvalid { .. })
        ));
    }
}

#[test]
fn empty_database_applies_foundation_and_reopen_is_a_no_op() {
    let database = TestDatabase::empty();
    let first = run_registry(&database, &FOUNDATION_MIGRATION).expect("first migration run");
    assert_eq!(first.schema_version, 1);
    assert_eq!(first.applied_count, 1);

    let connection = database.open_raw();
    let metadata: (i64, String, String, i64) = connection
        .query_row(
            "SELECT version, name, checksum_sha256, applied_at_ms
             FROM schema_migrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read migration metadata");
    assert_eq!(metadata.0, 1);
    assert_eq!(metadata.1, "foundation");
    assert_eq!(metadata.2, FOUNDATION_MIGRATION[0].checksum_sha256());
    assert!(metadata.3 >= 0);
    let schema_before: String = connection
        .query_row(
            "SELECT group_concat(name || ':' || sql, char(10))
             FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name",
            [],
            |row| row.get(0),
        )
        .expect("read schema snapshot");
    drop(connection);

    let second = run_registry(&database, &FOUNDATION_MIGRATION).expect("second migration run");
    assert_eq!(second.schema_version, 1);
    assert_eq!(second.applied_count, 0);
    let connection = database.open_raw();
    let schema_after: String = connection
        .query_row(
            "SELECT group_concat(name || ':' || sql, char(10))
             FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name",
            [],
            |row| row.get(0),
        )
        .expect("read schema snapshot after reopen");
    assert_eq!(schema_after, schema_before);
    assert_eq!(count_rows(&connection, "schema_migrations"), 1);
    let metadata_after: (i64, String, String, i64) = connection
        .query_row(
            "SELECT version, name, checksum_sha256, applied_at_ms
             FROM schema_migrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read metadata after reopen");
    assert_eq!(metadata_after, metadata);
}

#[test]
fn changed_checksum_and_name_are_rejected_without_automatic_repair() {
    static ORIGINAL: [Migration; 1] = [Migration::new(
        1,
        "original",
        "CREATE TABLE original_table (id INTEGER PRIMARY KEY);",
    )];
    static CHANGED_SQL: [Migration; 1] = [Migration::new(
        1,
        "original",
        "CREATE TABLE changed_table (id INTEGER PRIMARY KEY);",
    )];
    static CHANGED_NAME: [Migration; 1] = [Migration::new(
        1,
        "renamed",
        "CREATE TABLE original_table (id INTEGER PRIMARY KEY);",
    )];

    let checksum_database = TestDatabase::empty();
    run_registry(&checksum_database, &ORIGINAL).expect("apply original migration");
    assert!(matches!(
        run_registry(&checksum_database, &CHANGED_SQL),
        Err(DatabaseError::MigrationChecksumMismatch { version: 1 })
    ));

    let name_database = TestDatabase::empty();
    run_registry(&name_database, &ORIGINAL).expect("apply original migration");
    assert!(matches!(
        run_registry(&name_database, &CHANGED_NAME),
        Err(DatabaseError::MigrationMetadataInvalid { .. })
    ));
}

#[test]
fn missing_history_and_newer_database_are_rejected() {
    static ONE: [Migration; 1] = [Migration::new(
        1,
        "one",
        "CREATE TABLE migration_one (id INTEGER PRIMARY KEY);",
    )];
    static TWO: [Migration; 2] = [
        Migration::new(
            1,
            "one",
            "CREATE TABLE migration_one (id INTEGER PRIMARY KEY);",
        ),
        Migration::new(
            2,
            "two",
            "CREATE TABLE migration_two (id INTEGER PRIMARY KEY);",
        ),
    ];

    let missing_database = TestDatabase::empty();
    run_registry(&missing_database, &TWO).expect("apply two migrations");
    missing_database
        .open_raw()
        .execute("DELETE FROM schema_migrations WHERE version = 1", [])
        .expect("remove first metadata row");
    assert!(matches!(
        run_registry(&missing_database, &TWO),
        Err(DatabaseError::MigrationOutOfOrder {
            expected: 1,
            found: 2
        })
    ));

    let newer_database = TestDatabase::empty();
    run_registry(&newer_database, &TWO).expect("apply newer schema");
    assert!(matches!(
        run_registry(&newer_database, &ONE),
        Err(DatabaseError::DatabaseNewerThanApplication {
            database_version: 2,
            application_version: 1
        })
    ));
}

#[test]
fn failed_sql_rolls_back_current_migration_but_keeps_prior_commits() {
    static REGISTRY: [Migration; 2] = [
        Migration::new(
            1,
            "good",
            "CREATE TABLE committed_table (id INTEGER PRIMARY KEY);",
        ),
        Migration::new(
            2,
            "bad",
            "CREATE TABLE rolled_back_table (id INTEGER PRIMARY KEY); INVALID SQL;",
        ),
    ];

    let database = TestDatabase::empty();
    assert!(matches!(
        run_registry(&database, &REGISTRY),
        Err(DatabaseError::MigrationExecutionFailed { version: 2, .. })
    ));
    let connection = database.open_raw();
    assert!(table_exists(&connection, "committed_table"));
    assert!(!table_exists(&connection, "rolled_back_table"));
    assert_eq!(count_rows(&connection, "schema_migrations"), 1);
    let version: i64 = connection
        .query_row("SELECT version FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .expect("read committed version");
    assert_eq!(version, 1);
}

#[test]
fn metadata_insert_failure_rolls_back_schema_changes() {
    static REGISTRY: [Migration; 1] = [Migration::new(
        1,
        "metadata_failure",
        "CREATE TABLE metadata_rollback_table (id INTEGER PRIMARY KEY);
         CREATE TRIGGER reject_migration_metadata
         BEFORE INSERT ON schema_migrations
         BEGIN
             SELECT RAISE(ABORT, 'metadata insert rejected');
         END;",
    )];

    let database = TestDatabase::empty();
    assert!(matches!(
        run_registry(&database, &REGISTRY),
        Err(DatabaseError::MigrationExecutionFailed { version: 1, .. })
    ));
    let connection = database.open_raw();
    assert!(!table_exists(&connection, "metadata_rollback_table"));
    assert!(!table_exists(&connection, "reject_migration_metadata"));
    assert_eq!(count_rows(&connection, "schema_migrations"), 0);
}

#[test]
fn foreign_key_check_failure_rolls_back_schema_and_metadata() {
    static REGISTRY: [Migration; 1] = [Migration::new(
        1,
        "foreign_key_failure",
        "CREATE TABLE migration_parent (id INTEGER PRIMARY KEY);
         CREATE TABLE migration_child (
             id INTEGER PRIMARY KEY,
             parent_id INTEGER NOT NULL,
             FOREIGN KEY (parent_id) REFERENCES migration_parent (id)
                 DEFERRABLE INITIALLY DEFERRED
         );
         INSERT INTO migration_child (id, parent_id) VALUES (1, 999);",
    )];

    let database = TestDatabase::empty();
    assert!(matches!(
        run_registry(&database, &REGISTRY),
        Err(DatabaseError::ForeignKeyViolation {
            version: 1,
            violations: 1
        })
    ));
    let connection = database.open_raw();
    assert!(!table_exists(&connection, "migration_parent"));
    assert!(!table_exists(&connection, "migration_child"));
    assert_eq!(count_rows(&connection, "schema_migrations"), 0);
}

#[test]
fn malformed_existing_metadata_table_is_rejected() {
    let database = TestDatabase::empty();
    database
        .open_raw()
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                checksum_sha256 TEXT NOT NULL,
                applied_at_ms INTEGER NOT NULL
             );",
        )
        .expect("create malformed metadata table");
    assert!(matches!(
        run_registry(&database, &FOUNDATION_MIGRATION),
        Err(DatabaseError::MigrationMetadataInvalid { .. })
    ));
}

#[test]
fn foundation_contains_required_tables_and_indexes() {
    let database = TestDatabase::migrated();
    let connection = database.open_raw();
    for table in [
        "projects",
        "app_profiles",
        "provider_bindings",
        "tasks",
        "task_state_transitions",
        "active_task_leases",
        "schema_migrations",
    ] {
        assert!(table_exists(&connection, table), "missing table {table}");
    }

    let indexes = [
        "tasks_project_id_idx",
        "tasks_state_idx",
        "provider_bindings_app_profile_id_idx",
    ];
    for index in indexes {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_schema WHERE type = 'index' AND name = ?1
                 )",
                [index],
                |row| row.get(0),
            )
            .expect("query index");
        assert!(exists, "missing index {index}");
    }
}

fn assert_task_insert_rejected(
    connection: &mut Connection,
    state: &str,
    resume_target: Option<&str>,
    terminal_at_ms: Option<i64>,
) {
    let transaction = connection.transaction().expect("begin rejected insert");
    let error = insert_task(
        &transaction,
        "rejected-task",
        "project",
        state,
        0,
        resume_target,
        terminal_at_ms,
    )
    .expect_err("task insert must be rejected");
    assert!(is_constraint_error(&error), "unexpected error: {error:?}");
    transaction.rollback().expect("rollback rejected insert");
}

#[test]
fn task_state_resume_target_and_terminal_timestamp_checks_are_enforced() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project");

    assert_task_insert_rejected(&mut connection, "NotAState", None, None);
    assert_task_insert_rejected(&mut connection, "Paused", None, None);
    assert_task_insert_rejected(&mut connection, "Testing", Some("Testing"), None);
    assert_task_insert_rejected(
        &mut connection,
        "UnknownExternalEffect",
        Some("Testing"),
        None,
    );
    assert_task_insert_rejected(&mut connection, "Completed", None, None);
    assert_task_insert_rejected(&mut connection, "Testing", None, Some(100));
    assert_task_insert_rejected(&mut connection, "CleanupPending", None, None);
    assert_task_insert_rejected(&mut connection, "Archived", None, None);

    for (index, target) in [None, Some("Testing")].into_iter().enumerate() {
        let task_id = format!("recovery-{index}");
        let transaction = connection.transaction().expect("begin recovery insert");
        insert_task(
            &transaction,
            &task_id,
            "project",
            "RecoveryRequired",
            1,
            target,
            None,
        )
        .expect("RecoveryRequired target policy");
        insert_lease(&transaction, &task_id).expect("insert recovery lease");
        transaction.commit().expect("commit recovery task");
        let transaction = connection
            .transaction()
            .expect("begin recovery terminal transition");
        transaction
            .execute(
                "UPDATE tasks
                 SET state = 'Failed', resume_target_state = NULL, terminal_at_ms = 100
                 WHERE id = ?1",
                [&task_id],
            )
            .expect("make recovery task terminal");
        transaction
            .execute(
                "DELETE FROM active_task_leases WHERE task_id = ?1",
                [&task_id],
            )
            .expect("release recovery task lease");
        transaction
            .commit()
            .expect("commit recovery terminal transition");
    }
}

#[test]
fn immutable_task_columns_are_guarded_by_triggers() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project");
    insert_project(&connection, "other-project");
    create_active_task(&mut connection, "immutable-task", "project");

    for sql in [
        "UPDATE tasks SET project_id = 'other-project' WHERE id = 'immutable-task'",
        "UPDATE tasks SET task_branch_identity = 'ai-task/other' WHERE id = 'immutable-task'",
        "UPDATE tasks SET created_at_ms = 99 WHERE id = 'immutable-task'",
    ] {
        let error = connection.execute(sql, []).expect_err("update must fail");
        assert!(is_constraint_error(&error), "unexpected error: {error:?}");
    }
}

#[test]
fn transition_shape_sequence_and_code_lengths_are_enforced() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project");
    create_active_task(&mut connection, "transition-task", "project");

    let cases = [
        (0, Some("Created"), "ProjectValidated", 1, "actor", "reason"),
        (1, Some("Created"), "Created", 0, "actor", "reason"),
        (2, None, "ProjectValidated", 1, "actor", "reason"),
        (
            2,
            Some("NotAState"),
            "ProjectValidated",
            1,
            "actor",
            "reason",
        ),
        (2, Some("Created"), "NotAState", 1, "actor", "reason"),
        (2, Some("Created"), "ProjectValidated", 1, "", "reason"),
        (2, Some("Created"), "ProjectValidated", 1, "actor", ""),
    ];
    for (index, (sequence, from, to, version, actor, reason)) in cases.into_iter().enumerate() {
        let error = connection
            .execute(
                "INSERT INTO task_state_transitions (
                    id, task_id, sequence, from_state, to_state, task_version,
                    actor_kind, reason_code, occurred_at_ms
                 ) VALUES (?1, 'transition-task', ?2, ?3, ?4, ?5, ?6, ?7, 100)",
                params![
                    format!("invalid-transition-{index}"),
                    sequence,
                    from,
                    to,
                    version,
                    actor,
                    reason
                ],
            )
            .expect_err("invalid transition must fail");
        assert!(is_constraint_error(&error), "unexpected error: {error:?}");
    }

    for (index, (actor, reason)) in [
        ("a".repeat(65), "reason".to_owned()),
        ("actor".to_owned(), "r".repeat(129)),
    ]
    .into_iter()
    .enumerate()
    {
        let error = connection
            .execute(
                "INSERT INTO task_state_transitions (
                    id, task_id, sequence, from_state, to_state, task_version,
                    actor_kind, reason_code, occurred_at_ms
                 ) VALUES (?1, 'transition-task', 2, 'Created', 'ProjectValidated', 1, ?2, ?3, 100)",
                params![format!("long-code-{index}"), actor, reason],
            )
            .expect_err("overlong code must fail");
        assert!(is_constraint_error(&error), "unexpected error: {error:?}");
    }
}

#[test]
fn active_task_lifecycle_and_singleton_lease_are_enforced() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project");
    create_active_task(&mut connection, "active-task", "project");
    assert_eq!(count_rows(&connection, "tasks"), 1);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 1);
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);

    let transaction = connection.transaction().expect("begin second task");
    insert_task(
        &transaction,
        "second-task",
        "project",
        "Created",
        0,
        None,
        None,
    )
    .expect("insert second task");
    let singleton_error =
        insert_lease(&transaction, "second-task").expect_err("second singleton lease must fail");
    assert!(is_constraint_error(&singleton_error));
    transaction.rollback().expect("rollback second task");

    let transaction = connection.transaction().expect("begin lease-less task");
    insert_task(
        &transaction,
        "lease-less-task",
        "project",
        "Created",
        0,
        None,
        None,
    )
    .expect("insert task without lease");
    let commit_error = transaction
        .commit()
        .expect_err("active task without lease must fail at commit");
    assert!(is_constraint_error(&commit_error));

    let transaction = connection.transaction().expect("begin terminal transition");
    transaction
        .execute(
            "UPDATE tasks
             SET state = 'Completed', version = 1, updated_at_ms = 110, terminal_at_ms = 110
             WHERE id = 'active-task'",
            [],
        )
        .expect("update task to terminal");
    transaction
        .execute(
            "INSERT INTO task_state_transitions (
                id, task_id, sequence, from_state, to_state, task_version,
                actor_kind, reason_code, occurred_at_ms
             ) VALUES (
                'transition-active-task-2', 'active-task', 2, 'Created', 'Completed', 1,
                'application', 'task.completed', 110
             )",
            [],
        )
        .expect("insert terminal transition");
    transaction
        .execute(
            "DELETE FROM active_task_leases WHERE task_id = 'active-task'",
            [],
        )
        .expect("delete terminal lease after state update");
    transaction.commit().expect("commit terminal lifecycle");

    assert_eq!(count_rows(&connection, "active_task_leases"), 0);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 2);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

#[test]
fn lease_delete_order_and_terminal_insert_are_guarded() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project");
    create_active_task(&mut connection, "delete-guard-task", "project");

    let error = connection
        .execute(
            "DELETE FROM active_task_leases WHERE task_id = 'delete-guard-task'",
            [],
        )
        .expect_err("active lease cannot be deleted first");
    assert!(is_constraint_error(&error));

    let transaction = connection
        .transaction()
        .expect("begin guard task terminal transition");
    transaction
        .execute(
            "UPDATE tasks
             SET state = 'Failed', version = 1, terminal_at_ms = 100
             WHERE id = 'delete-guard-task'",
            [],
        )
        .expect("make guard task terminal");
    transaction
        .execute(
            "DELETE FROM active_task_leases WHERE task_id = 'delete-guard-task'",
            [],
        )
        .expect("delete guard task lease after state update");
    transaction
        .commit()
        .expect("commit guard task terminal state");

    let transaction = connection
        .transaction()
        .expect("begin terminal task insert");
    insert_task(
        &transaction,
        "terminal-task",
        "project",
        "Failed",
        1,
        None,
        Some(100),
    )
    .expect("insert terminal task");
    transaction.commit().expect("commit terminal task");
    let error = connection
        .execute(
            "INSERT INTO active_task_leases (singleton_key, task_id, acquired_at_ms)
             VALUES (1, 'terminal-task', 100)",
            [],
        )
        .expect_err("terminal lease insert must fail");
    assert!(is_constraint_error(&error));
    assert_eq!(foreign_key_violation_count(&connection), 0);
}
