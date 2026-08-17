use std::sync::{Arc, Mutex};

use chatoms_infrastructure::bootstrap::{
    DatabaseBootstrapAdapter, LoggingBootstrapAdapter, SharedDatabase, SharedLoggingGuard,
};
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState, LoggingBootstrapPort, LoggingBootstrapState,
    error::{CategorizedFailure, FailureCategory},
    path::ResolvedAppPaths,
    repository::FoundationRepository,
};
use tempfile::TempDir;

fn paths(temp: &TempDir) -> ResolvedAppPaths {
    let app_root = temp.path().join("ChatOMS");
    std::fs::create_dir_all(app_root.join("data")).expect("data directory");
    std::fs::create_dir_all(app_root.join("logs")).expect("logs directory");
    ResolvedAppPaths {
        data_dir: app_root.join("data"),
        database_path: app_root.join("data/chatoms.sqlite3"),
        logs_dir: app_root.join("logs"),
        artifacts_dir: app_root.join("artifacts"),
        temp_dir: app_root.join("temp"),
        worktrees_dir: app_root.join("worktrees"),
        app_root,
    }
}

#[test]
fn database_bootstrap_migrates_reopens_and_exposes_shared_repository() {
    let temp = TempDir::new().expect("temp");
    let resolved = paths(&temp);
    let shared_paths = Arc::new(Mutex::new(Some(resolved.clone())));
    let database = SharedDatabase::default();
    let mut adapter = DatabaseBootstrapAdapter::new(shared_paths.clone(), database.clone());
    assert_eq!(
        adapter.bootstrap_database().expect("first bootstrap"),
        DatabaseBootstrapState::Upgraded
    );
    assert!(database.is_initialized());
    assert!(
        database
            .repository()
            .list_projects()
            .expect("projects")
            .is_empty()
    );

    let reopened = SharedDatabase::default();
    let mut adapter = DatabaseBootstrapAdapter::new(shared_paths, reopened.clone());
    assert_eq!(
        adapter.bootstrap_database().expect("reopen"),
        DatabaseBootstrapState::Ready
    );
    assert!(reopened.is_initialized());
}

#[test]
fn newer_database_is_incompatible_and_failures_are_safe() {
    let temp = TempDir::new().expect("temp");
    let resolved = paths(&temp);
    let shared_paths = Arc::new(Mutex::new(Some(resolved.clone())));
    let mut first = DatabaseBootstrapAdapter::new(shared_paths.clone(), SharedDatabase::default());
    assert_eq!(
        first.bootstrap_database().expect("foundation"),
        DatabaseBootstrapState::Upgraded
    );
    let connection =
        rusqlite::Connection::open(&resolved.database_path).expect("second connection");
    connection
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum_sha256, applied_at_ms)
             VALUES (21, 'future', ?1, 1)",
            ["0".repeat(64)],
        )
        .expect("future migration marker");
    drop(connection);

    let mut incompatible = DatabaseBootstrapAdapter::new(shared_paths, SharedDatabase::default());
    assert_eq!(
        incompatible.bootstrap_database().expect("status"),
        DatabaseBootstrapState::Incompatible
    );

    let missing_paths = Arc::new(Mutex::new(None));
    let mut missing = DatabaseBootstrapAdapter::new(missing_paths, SharedDatabase::default());
    let error = missing
        .bootstrap_database()
        .expect_err("missing secure paths");
    assert_eq!(error.category(), FailureCategory::StorageUnavailable);
    assert!(!error.to_string().contains("C:\\"));
    assert!(!error.to_string().contains("SELECT"));
}

#[test]
fn logging_bootstrap_retains_guard_rejects_duplicate_and_has_no_raw_fallback() {
    let temp = TempDir::new().expect("temp");
    let resolved = paths(&temp);
    let shared_paths = Arc::new(Mutex::new(Some(resolved.clone())));
    let guard = SharedLoggingGuard::default();
    let mut adapter = LoggingBootstrapAdapter::new(shared_paths.clone(), guard.clone());
    assert_eq!(
        adapter.bootstrap_logging().expect("logging"),
        LoggingBootstrapState::Ready
    );
    assert!(guard.is_initialized());

    let mut duplicate = LoggingBootstrapAdapter::new(shared_paths, SharedLoggingGuard::default());
    let error = duplicate
        .bootstrap_logging()
        .expect_err("duplicate global subscriber");
    assert_eq!(error.category(), FailureCategory::LoggingFailure);
    assert_eq!(error.to_string(), "FAILURE_LOGGING");

    let invalid_root = temp.path().join("other");
    let invalid = ResolvedAppPaths {
        logs_dir: invalid_root,
        ..resolved
    };
    let mut insecure = LoggingBootstrapAdapter::new(
        Arc::new(Mutex::new(Some(invalid))),
        SharedLoggingGuard::default(),
    );
    assert_eq!(
        insecure
            .bootstrap_logging()
            .expect_err("invalid secure path")
            .category(),
        FailureCategory::LoggingFailure
    );
}
