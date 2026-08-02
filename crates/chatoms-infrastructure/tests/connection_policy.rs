use chatoms_infrastructure::database::{DatabaseConnection, DatabaseError, PragmaSettings};

fn open_temp_database() -> (tempfile::TempDir, DatabaseConnection) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let database = DatabaseConnection::open(directory.path().join("chatoms.sqlite3"))
        .expect("open configured file database");
    (directory, database)
}

#[test]
fn new_file_database_uses_required_connection_policy() {
    let (_directory, database) = open_temp_database();
    let settings = database.verify().expect("verify connection policy");
    println!("SQLite {} PRAGMAs: {settings:?}", rusqlite::version());
    assert_eq!(
        settings,
        PragmaSettings {
            foreign_keys: 1,
            journal_mode: "wal".to_owned(),
            synchronous: 2,
            busy_timeout_ms: 5_000,
        }
    );
}

#[test]
fn configuring_the_same_connection_again_is_idempotent() {
    let (_directory, mut database) = open_temp_database();
    let before = database.verify().expect("initial settings");
    database.configure().expect("reconfigure connection");
    let after = database.verify().expect("settings after reconfigure");
    assert_eq!(after, before);
}

#[test]
fn production_open_rejects_in_memory_database_identifiers() {
    for path in [
        ":memory:",
        "file::memory:",
        "file::memory:?cache=shared",
        "file:chatoms?mode=memory",
    ] {
        assert!(matches!(
            DatabaseConnection::open(path),
            Err(DatabaseError::InvariantViolation { .. })
        ));
    }
}

#[test]
fn database_open_failure_is_a_typed_error() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    assert!(matches!(
        DatabaseConnection::open(directory.path()),
        Err(DatabaseError::OpenDatabase(_))
    ));
}
