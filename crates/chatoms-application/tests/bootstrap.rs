mod support;

use std::sync::{Arc, Mutex};

use chatoms_application::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapService, DatabaseStatus, LoggingStatus, StorageStatus},
    error::ApplicationErrorCode,
};
use chatoms_domain::TaskId;
use chatoms_ports::{
    DatabaseBootstrapPort, DatabaseBootstrapState, LoggingBootstrapPort, LoggingBootstrapState,
    StorageBootstrapPort, StorageBootstrapState,
    error::{FailureCategory, PortFailure},
    repository::ActiveLease,
};

use support::FakeRepository;

type Calls = Arc<Mutex<Vec<&'static str>>>;

struct StorageFake {
    calls: Calls,
    result: Result<StorageBootstrapState, PortFailure>,
}

impl StorageBootstrapPort for StorageFake {
    fn prepare_secure_storage(&mut self) -> Result<StorageBootstrapState, PortFailure> {
        self.calls.lock().expect("calls").push("storage");
        self.result
    }
}

struct DatabaseFake {
    calls: Calls,
    result: Result<DatabaseBootstrapState, PortFailure>,
}

impl DatabaseBootstrapPort for DatabaseFake {
    fn bootstrap_database(&mut self) -> Result<DatabaseBootstrapState, PortFailure> {
        self.calls.lock().expect("calls").push("database");
        self.result
    }
}

struct LoggingFake {
    calls: Calls,
    result: Result<LoggingBootstrapState, PortFailure>,
}

impl LoggingBootstrapPort for LoggingFake {
    fn bootstrap_logging(&mut self) -> Result<LoggingBootstrapState, PortFailure> {
        self.calls.lock().expect("calls").push("logging");
        self.result
    }
}

fn run(
    storage_result: Result<StorageBootstrapState, PortFailure>,
    database_result: Result<DatabaseBootstrapState, PortFailure>,
    logging_result: Result<LoggingBootstrapState, PortFailure>,
    lease: Option<ActiveLease>,
) -> (
    Result<
        chatoms_application::bootstrap::BootstrapStatus,
        chatoms_application::error::ApplicationError,
    >,
    Vec<&'static str>,
) {
    let calls = Calls::default();
    let mut storage = StorageFake {
        calls: calls.clone(),
        result: storage_result,
    };
    let mut database = DatabaseFake {
        calls: calls.clone(),
        result: database_result,
    };
    let mut logging = LoggingFake {
        calls: calls.clone(),
        result: logging_result,
    };
    let mut repository = FakeRepository {
        active_lease: lease,
        shared_calls: Some(calls.clone()),
        ..FakeRepository::default()
    };
    let result = BootstrapService::new(&mut storage, &mut database, &mut logging, &mut repository)
        .bootstrap();
    let recorded = calls.lock().expect("calls").clone();
    (result, recorded)
}

#[test]
fn successful_bootstrap_is_ordered_ready_and_supports_absent_or_active_lease() {
    for lease in [
        None,
        Some(ActiveLease {
            task_id: TaskId::new(),
            acquired_at_ms: 42,
        }),
    ] {
        let (result, calls) = run(
            Ok(StorageBootstrapState::Ready),
            Ok(DatabaseBootstrapState::Ready),
            Ok(LoggingBootstrapState::Ready),
            lease,
        );
        let status = result.expect("bootstrap");
        assert_eq!(calls, ["storage", "database", "logging", "active_lease"]);
        assert!(status.ready);
        assert_eq!(status.application_version, APPLICATION_VERSION);
        assert_eq!(status.storage_status, StorageStatus::Ready);
        assert_eq!(status.database_status, DatabaseStatus::Ready);
        assert_eq!(status.logging_status, LoggingStatus::Ready);
        match (lease, status.active_task_status) {
            (None, ActiveTaskStatus::None) => {}
            (
                Some(expected),
                ActiveTaskStatus::Active {
                    task_id,
                    acquired_at_ms,
                },
            ) => {
                assert_eq!(task_id, expected.task_id);
                assert_eq!(acquired_at_ms, expected.acquired_at_ms);
            }
            other => panic!("unexpected active status: {other:?}"),
        }
    }
}

#[test]
fn insecure_unavailable_and_unsupported_storage_stop_all_later_ports() {
    for (port, expected) in [
        (StorageBootstrapState::Insecure, StorageStatus::Insecure),
        (
            StorageBootstrapState::Unavailable,
            StorageStatus::Unavailable,
        ),
        (
            StorageBootstrapState::Unsupported,
            StorageStatus::Unsupported,
        ),
    ] {
        let (result, calls) = run(
            Ok(port),
            Ok(DatabaseBootstrapState::Ready),
            Ok(LoggingBootstrapState::Ready),
            None,
        );
        let status = result.expect("status, not adapter error");
        assert_eq!(calls, ["storage"]);
        assert_eq!(status.storage_status, expected);
        assert_eq!(status.database_status, DatabaseStatus::NotChecked);
        assert!(!status.ready);
    }
}

#[test]
fn database_non_ready_states_stop_logging_and_active_lease_lookup() {
    for (port, expected) in [
        (
            DatabaseBootstrapState::Unavailable,
            DatabaseStatus::Unavailable,
        ),
        (
            DatabaseBootstrapState::Incompatible,
            DatabaseStatus::Incompatible,
        ),
        (
            DatabaseBootstrapState::MigrationRequired,
            DatabaseStatus::MigrationRequired,
        ),
    ] {
        let (result, calls) = run(
            Ok(StorageBootstrapState::Ready),
            Ok(port),
            Ok(LoggingBootstrapState::Ready),
            None,
        );
        let status = result.expect("database status");
        assert_eq!(calls, ["storage", "database"]);
        assert_eq!(status.database_status, expected);
        assert_eq!(status.logging_status, LoggingStatus::NotChecked);
        assert!(!status.ready);
    }
}

#[test]
fn upgraded_database_is_ready_and_logging_unavailable_is_degraded_not_raw_fallback() {
    for logging in [
        Ok(LoggingBootstrapState::Unavailable),
        Err(PortFailure::new(FailureCategory::LoggingFailure)),
    ] {
        let (result, calls) = run(
            Ok(StorageBootstrapState::Ready),
            Ok(DatabaseBootstrapState::Upgraded),
            logging,
            None,
        );
        let status = result.expect("degraded logging remains bootstrapped");
        assert_eq!(calls, ["storage", "database", "logging", "active_lease"]);
        assert_eq!(status.database_status, DatabaseStatus::Upgraded);
        assert_eq!(status.logging_status, LoggingStatus::Unavailable);
        assert!(status.ready);
    }
}

#[test]
fn port_failures_map_safely_and_stop_following_calls() {
    let (storage_result, storage_calls) = run(
        Err(PortFailure::new(FailureCategory::StorageUnavailable)),
        Ok(DatabaseBootstrapState::Ready),
        Ok(LoggingBootstrapState::Ready),
        None,
    );
    let error = storage_result.expect_err("storage failure");
    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
    assert_eq!(storage_calls, ["storage"]);

    let (database_result, database_calls) = run(
        Ok(StorageBootstrapState::Ready),
        Err(PortFailure::new(FailureCategory::MigrationFailure)),
        Ok(LoggingBootstrapState::Ready),
        None,
    );
    let error = database_result.expect_err("database failure");
    assert_eq!(error.code(), ApplicationErrorCode::MigrationFailed);
    assert_eq!(database_calls, ["storage", "database"]);
    let displayed = error.to_string();
    for forbidden in ["C:\\", "S-1-5-", "SELECT", "secret"] {
        assert!(!displayed.contains(forbidden));
    }
}
