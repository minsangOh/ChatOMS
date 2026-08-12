mod support;

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use chatoms_infrastructure::bootstrap::LegacyProjectPreflightAdapter;
use chatoms_infrastructure::database::{
    DatabaseConnection, DatabaseError, FOUNDATION_MIGRATION, LegacyProject, LegacyProjectIdentity,
    LegacyProjectPreflight, Migration, MigrationRunner, checksum_sha256, validate_registry,
};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    filesystem::{DirectoryIdentity, DirectoryIdentityGuard, FilesystemIdentityPort},
    git::{
        GitService, ProjectInspection, RepositoryKind, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
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
    assert_eq!(FOUNDATION_MIGRATION.len(), 3);
    assert_eq!(FOUNDATION_MIGRATION[0].version, 1);
    assert_eq!(FOUNDATION_MIGRATION[0].name, "foundation");
    assert_eq!(FOUNDATION_MIGRATION[1].version, 2);
    assert_eq!(FOUNDATION_MIGRATION[1].name, "git_isolation");
    assert_eq!(FOUNDATION_MIGRATION[2].version, 3);
    assert_eq!(FOUNDATION_MIGRATION[2].name, "provider_binding");
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
fn phase2_schema_enforces_canonical_project_and_git_intent_invariants() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project-one");
    let duplicate = connection.execute(
        "INSERT INTO projects (id, name, root_path, canonical_path_key, display_path, created_at_ms, updated_at_ms)
         VALUES ('project-two', 'Two', 'C:/OTHER', 'c:/project/project-one', 'Two', 100, 100)",
        [],
    );
    assert!(duplicate.as_ref().is_err_and(is_constraint_error));
    assert!(
        connection
            .execute(
                "UPDATE projects SET root_path = 'C:/moved' WHERE id = 'project-one'",
                []
            )
            .as_ref()
            .is_err_and(is_constraint_error)
    );

    create_active_task(&mut connection, "task-one", "project-one");
    let invalid_ready = connection.execute(
        "INSERT INTO task_git_isolations (
            task_id, project_id, status, expected_task_version,
            branch_created_by_app, worktree_created_by_app, created_at_ms, updated_at_ms
         ) VALUES ('task-one', 'project-one', 'WorktreeReady', 0, 1, 1, 100, 100)",
        [],
    );
    assert!(invalid_ready.as_ref().is_err_and(is_constraint_error));

    connection.execute(
        "INSERT INTO task_git_isolations (
            task_id, project_id, status, operation_id, expected_task_version,
            branch_created_by_app, worktree_created_by_app, created_at_ms, updated_at_ms
         ) VALUES ('task-one', 'project-one', 'GitInitInProgress', 'operation-one', 0, 0, 0, 100, 100)",
        [],
    ).expect("valid isolation intent");
    let stale_approval = connection.execute(
        "INSERT INTO git_init_approvals (operation_id, task_id, project_id, approved_task_version, approved_at_ms)
         VALUES ('operation-one', 'task-one', 'project-one', 1, 100)",
        [],
    );
    assert!(stale_approval.as_ref().is_err_and(is_constraint_error));
    connection.execute(
        "INSERT INTO git_init_approvals (operation_id, task_id, project_id, approved_task_version, approved_at_ms)
         VALUES ('operation-one', 'task-one', 'project-one', 0, 100)",
        [],
    ).expect("approval bound to exact intent");
    assert!(
        connection
            .execute(
                "UPDATE git_init_approvals SET approved_task_version = 1",
                []
            )
            .as_ref()
            .is_err_and(is_constraint_error)
    );
}

#[test]
fn isolation_status_truth_table_is_enforced_by_sql_for_every_status() {
    let database = TestDatabase::migrated();
    let mut connection = database.open_raw();
    insert_project(&connection, "project");
    create_active_task(&mut connection, "task", "project");
    let valid = [
        ("AwaitingGitInitApproval", None, None, 0, 0),
        ("Ready", None, None, 0, 0),
        ("Ready", Some("operation"), None, 0, 0),
        ("GitInitInProgress", Some("operation"), None, 0, 0),
        ("WorktreeCreating", Some("operation"), Some("base"), 0, 0),
        ("WorktreeReady", Some("operation"), Some("base"), 1, 1),
        ("RecoveryRequired", Some("operation"), None, 0, 0),
        ("RecoveryRequired", Some("operation"), Some("base"), 0, 0),
    ];
    for (status, operation, base, branch_owned, worktree_owned) in valid {
        let (base_branch, base_commit, worktree_path) = if base.is_some() {
            (Some("main"), Some("a".repeat(40)), Some("C:/managed/task"))
        } else {
            (None, None, None)
        };
        connection
            .execute(
                "INSERT INTO task_git_isolations (
                    task_id, project_id, status, operation_id, expected_task_version,
                    base_branch, base_commit, worktree_path, branch_created_by_app,
                    worktree_created_by_app, created_at_ms, updated_at_ms
                 ) VALUES ('task', 'project', ?1, ?2, 0, ?3, ?4, ?5, ?6, ?7, 100, 100)",
                params![
                    status,
                    operation,
                    base_branch,
                    base_commit,
                    worktree_path,
                    branch_owned,
                    worktree_owned
                ],
            )
            .unwrap_or_else(|error| panic!("valid status {status} rejected: {error}"));
        connection
            .execute("DELETE FROM task_git_isolations WHERE task_id = 'task'", [])
            .expect("delete truth-table fixture");
    }

    for (status, operation, base, branch_owned, worktree_owned) in [
        ("AwaitingGitInitApproval", Some("operation"), None, 0, 0),
        ("GitInitInProgress", None, None, 0, 0),
        ("WorktreeCreating", Some("operation"), None, 0, 0),
        ("WorktreeReady", Some("operation"), Some("base"), 0, 1),
        ("RecoveryRequired", None, None, 0, 0),
        ("RecoveryRequired", Some("operation"), Some("base"), 1, 1),
    ] {
        let (base_branch, base_commit, worktree_path) = if base.is_some() {
            (Some("main"), Some("a".repeat(40)), Some("C:/managed/task"))
        } else {
            (None, None, None)
        };
        let result = connection.execute(
            "INSERT INTO task_git_isolations (
                task_id, project_id, status, operation_id, expected_task_version,
                base_branch, base_commit, worktree_path, branch_created_by_app,
                worktree_created_by_app, created_at_ms, updated_at_ms
             ) VALUES ('task', 'project', ?1, ?2, 0, ?3, ?4, ?5, ?6, ?7, 100, 100)",
            params![
                status,
                operation,
                base_branch,
                base_commit,
                worktree_path,
                branch_owned,
                worktree_owned
            ],
        );
        assert!(
            result.as_ref().is_err_and(is_constraint_error),
            "invalid status shape accepted: {status}"
        );
    }
}

struct StableLegacyPreflight {
    duplicate_identity: bool,
}

struct LegacyGitSpy {
    calls: Rc<RefCell<Vec<&'static str>>>,
}

impl GitService for LegacyGitSpy {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        Ok(true)
    }
    fn inspect_project(&mut self, _input: &Path) -> Result<ProjectInspection, PortFailure> {
        self.calls.borrow_mut().push("git.inspect");
        Ok(ProjectInspection {
            canonical_root: PathBuf::from("C:/canonical/project"),
            canonical_key: "c:/canonical/project".to_owned(),
            display_path: "…\\project".to_owned(),
            suggested_name: "project".to_owned(),
            confirmation_token: "migration".to_owned(),
            repository_kind: RepositoryKind::Git,
            repository_status: None,
            git_common_dir: Some(PathBuf::from("C:/canonical/project/.git")),
        })
    }
    fn repository_status(&mut self, _root: &Path) -> Result<RepositoryStatus, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn validate_non_git_source(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn validate_repository_source(
        &mut self,
        _root: &Path,
        _base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn initialize_repository(&mut self, _root: &Path) -> Result<(), PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn has_commit_author(&mut self, _root: &Path) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn create_initial_snapshot(&mut self, _root: &Path) -> Result<String, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn create_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
        _safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
    fn verify_task_worktree(
        &mut self,
        _root: &Path,
        _branch: &str,
        _base_commit: &str,
        _worktree: &Path,
    ) -> Result<bool, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

struct LegacyFilesystemSpy {
    calls: Rc<RefCell<Vec<&'static str>>>,
    fail_stored_root: bool,
    fail_detected_root: bool,
}

impl FilesystemIdentityPort for LegacyFilesystemSpy {
    fn inspect_supported_directory(
        &mut self,
        path: &Path,
    ) -> Result<DirectoryIdentity, PortFailure> {
        self.calls.borrow_mut().push("filesystem.inspect");
        if self.fail_stored_root
            || (self.fail_detected_root && path == Path::new("C:/canonical/project"))
        {
            return Err(PortFailure::new(FailureCategory::Unsupported));
        }
        Ok(DirectoryIdentity {
            canonical_path: path.to_path_buf(),
            volume_serial_hex: "volume".to_owned(),
            file_id_hex: path.to_string_lossy().into_owned(),
        })
    }
    fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
        self.calls.borrow_mut().push("filesystem.verify");
        Ok(())
    }
    fn acquire_guard(
        &mut self,
        _path: &Path,
        _expected: &DirectoryIdentity,
    ) -> Result<Box<dyn DirectoryIdentityGuard>, PortFailure> {
        Err(PortFailure::new(FailureCategory::Unsupported))
    }
}

#[test]
fn legacy_preflight_checks_stored_fixed_drive_policy_before_any_git_probe() {
    for root_path in ["C:/legacy/project", "Z:/mapped-network/project"] {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut preflight = LegacyProjectPreflightAdapter::new(
            LegacyGitSpy {
                calls: Rc::clone(&calls),
            },
            LegacyFilesystemSpy {
                calls: Rc::clone(&calls),
                fail_stored_root: true,
                fail_detected_root: false,
            },
        );
        let projects = [LegacyProject {
            project_id: "legacy".to_owned(),
            name: "Legacy".to_owned(),
            root_path: root_path.to_owned(),
        }];
        assert!(preflight.resolve(&projects).is_err());
        assert_eq!(*calls.borrow(), ["filesystem.inspect"]);
    }
}

#[test]
fn legacy_preflight_rechecks_detected_root_and_common_directory_after_git_probe() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut preflight = LegacyProjectPreflightAdapter::new(
        LegacyGitSpy {
            calls: Rc::clone(&calls),
        },
        LegacyFilesystemSpy {
            calls: Rc::clone(&calls),
            fail_stored_root: false,
            fail_detected_root: true,
        },
    );
    let projects = [LegacyProject {
        project_id: "legacy".to_owned(),
        name: "Legacy".to_owned(),
        root_path: "C:/legacy/project".to_owned(),
    }];
    assert!(preflight.resolve(&projects).is_err());
    assert_eq!(
        *calls.borrow(),
        [
            "filesystem.inspect",
            "filesystem.verify",
            "git.inspect",
            "filesystem.inspect"
        ]
    );
}

#[test]
fn detected_root_preflight_failure_rolls_back_populated_0001_upgrade() {
    let database = TestDatabase::empty();
    let mut connection = DatabaseConnection::open(database.path()).expect("open database");
    MigrationRunner::new(&FOUNDATION_MIGRATION[..1])
        .run(&mut connection)
        .expect("apply 0001 only");
    drop(connection);
    database
        .open_raw()
        .execute(
            "INSERT INTO projects (id, name, root_path, created_at_ms, updated_at_ms)
             VALUES ('legacy-root-failure', 'Legacy', 'C:/legacy/project', 100, 100)",
            [],
        )
        .expect("legacy project");
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut preflight = LegacyProjectPreflightAdapter::new(
        LegacyGitSpy {
            calls: Rc::clone(&calls),
        },
        LegacyFilesystemSpy {
            calls: Rc::clone(&calls),
            fail_stored_root: false,
            fail_detected_root: true,
        },
    );
    let mut connection = DatabaseConnection::open(database.path()).expect("reopen database");
    assert!(matches!(
        MigrationRunner::default().run_with_preflight(&mut connection, &mut preflight),
        Err(DatabaseError::LegacyProjectPreflightFailed { .. })
    ));
    assert!(calls.borrow().contains(&"git.inspect"));
    assert!(!table_exists(
        &database.open_raw(),
        "project_filesystem_identities"
    ));
}

impl LegacyProjectPreflight for StableLegacyPreflight {
    fn resolve(
        &mut self,
        projects: &[LegacyProject],
    ) -> Result<Vec<LegacyProjectIdentity>, DatabaseError> {
        Ok(projects
            .iter()
            .enumerate()
            .map(|(index, project)| LegacyProjectIdentity {
                project_id: project.project_id.clone(),
                canonical_path_key: format!("c:/legacy/{}", project.project_id),
                display_path: format!("legacy\\{}", project.name),
                root_volume_serial_hex: "0000000000000001".to_owned(),
                root_file_id_hex: if self.duplicate_identity {
                    "00000000000000000000000000000001".to_owned()
                } else {
                    format!("{index:032x}")
                },
                repository_kind: "NonGit",
                git_common_volume_serial_hex: None,
                git_common_file_id_hex: None,
            })
            .collect())
    }
}

#[test]
fn populated_0001_upgrade_requires_preflight_and_persists_confirmed_identity_atomically() {
    let database = TestDatabase::empty();
    let mut connection = DatabaseConnection::open(database.path()).expect("open database");
    MigrationRunner::new(&FOUNDATION_MIGRATION[..1])
        .run(&mut connection)
        .expect("apply 0001 only");
    drop(connection);
    database
        .open_raw()
        .execute(
            "INSERT INTO projects (id, name, root_path, created_at_ms, updated_at_ms)
             VALUES ('legacy-one', 'Legacy One', 'C:/legacy/one', 100, 100)",
            [],
        )
        .expect("legacy project");
    let mut connection = DatabaseConnection::open(database.path()).expect("reopen database");

    let error = MigrationRunner::default()
        .run(&mut connection)
        .expect_err("unconfirmed legacy identity must stop migration");
    assert!(matches!(
        error,
        DatabaseError::LegacyProjectPreflightFailed { .. }
    ));
    assert!(!table_exists(
        &database.open_raw(),
        "project_filesystem_identities"
    ));

    let mut preflight = StableLegacyPreflight {
        duplicate_identity: false,
    };
    MigrationRunner::default()
        .run_with_preflight(&mut connection, &mut preflight)
        .expect("confirmed legacy upgrade");
    let row: (String, String, i64) = database
        .open_raw()
        .query_row(
            "SELECT projects.display_path, identity.root_file_id_hex, identity.confirmed
             FROM projects
             JOIN project_filesystem_identities AS identity ON identity.project_id = projects.id
             WHERE projects.id = 'legacy-one'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("migrated identity");
    assert_eq!(row.0, "legacy\\Legacy One");
    assert_eq!(row.1, "00000000000000000000000000000000");
    assert_eq!(row.2, 1);
}

#[test]
fn duplicate_legacy_stable_identity_aborts_0002_without_partial_schema() {
    let database = TestDatabase::empty();
    let mut connection = DatabaseConnection::open(database.path()).expect("open database");
    MigrationRunner::new(&FOUNDATION_MIGRATION[..1])
        .run(&mut connection)
        .expect("apply 0001 only");
    drop(connection);
    let raw = database.open_raw();
    for (id, name) in [("legacy-one", "One"), ("legacy-two", "Two")] {
        raw.execute(
            "INSERT INTO projects (id, name, root_path, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 100, 100)",
            params![id, name, format!("C:/legacy/{id}")],
        )
        .expect("legacy project");
    }
    drop(raw);
    let mut connection = DatabaseConnection::open(database.path()).expect("reopen database");
    let mut preflight = StableLegacyPreflight {
        duplicate_identity: true,
    };
    assert!(matches!(
        MigrationRunner::default().run_with_preflight(&mut connection, &mut preflight),
        Err(DatabaseError::LegacyProjectPreflightFailed { .. })
    ));
    assert!(!table_exists(
        &database.open_raw(),
        "project_filesystem_identities"
    ));
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
    assert_eq!(first.schema_version, 3);
    assert_eq!(first.applied_count, 3);

    let connection = database.open_raw();
    let metadata: (i64, String, String, i64) = connection
        .query_row(
            "SELECT version, name, checksum_sha256, applied_at_ms
             FROM schema_migrations WHERE version = 3",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read migration metadata");
    assert_eq!(metadata.0, 3);
    assert_eq!(metadata.1, "provider_binding");
    assert_eq!(metadata.2, FOUNDATION_MIGRATION[2].checksum_sha256());
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
    assert_eq!(second.schema_version, 3);
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
    assert_eq!(count_rows(&connection, "schema_migrations"), 3);
    let metadata_after: (i64, String, String, i64) = connection
        .query_row(
            "SELECT version, name, checksum_sha256, applied_at_ms
             FROM schema_migrations WHERE version = 3",
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
