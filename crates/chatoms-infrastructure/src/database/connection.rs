use std::{path::Path, time::Duration};

use rusqlite::Connection;

use super::DatabaseError;

const BUSY_TIMEOUT: Duration = Duration::from_millis(5_000);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PragmaSettings {
    pub foreign_keys: i64,
    pub journal_mode: String,
    pub synchronous: i64,
    pub busy_timeout_ms: i64,
}

pub struct DatabaseConnection {
    connection: Connection,
}

impl DatabaseConnection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref();
        if !is_file_backed_path(path) {
            return Err(DatabaseError::InvariantViolation {
                reason: "production database must be file-backed",
            });
        }

        let connection = Connection::open(path).map_err(DatabaseError::OpenDatabase)?;
        let mut database = Self { connection };
        database.configure()?;
        database.verify()?;
        Ok(database)
    }

    pub fn configure(&mut self) -> Result<(), DatabaseError> {
        self.connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|source| DatabaseError::ConfigureDatabase {
                pragma: "foreign_keys",
                source,
            })?;
        self.connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|source| DatabaseError::ConfigureDatabase {
                pragma: "journal_mode",
                source,
            })?;
        self.connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|source| DatabaseError::ConfigureDatabase {
                pragma: "synchronous",
                source,
            })?;
        self.connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|source| DatabaseError::ConfigureDatabase {
                pragma: "busy_timeout",
                source,
            })?;
        Ok(())
    }

    pub fn verify(&self) -> Result<PragmaSettings, DatabaseError> {
        let settings = PragmaSettings {
            foreign_keys: query_i64(&self.connection, "foreign_keys")?,
            journal_mode: query_string(&self.connection, "journal_mode")?.to_ascii_lowercase(),
            synchronous: query_i64(&self.connection, "synchronous")?,
            busy_timeout_ms: query_i64(&self.connection, "busy_timeout")?,
        };

        verify_value("foreign_keys", "1", settings.foreign_keys.to_string())?;
        verify_value("journal_mode", "wal", settings.journal_mode.clone())?;
        verify_value("synchronous", "2", settings.synchronous.to_string())?;
        verify_value("busy_timeout", "5000", settings.busy_timeout_ms.to_string())?;
        Ok(settings)
    }

    pub(crate) fn raw_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

fn is_file_backed_path(path: &Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    let normalized = value.to_ascii_lowercase();
    !value.is_empty()
        && value != ":memory:"
        && !normalized.starts_with("file::memory:")
        && !normalized.contains("mode=memory")
}

fn query_i64(connection: &Connection, pragma: &'static str) -> Result<i64, DatabaseError> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .map_err(|source| DatabaseError::ConfigureDatabase { pragma, source })
}

fn query_string(connection: &Connection, pragma: &'static str) -> Result<String, DatabaseError> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .map_err(|source| DatabaseError::ConfigureDatabase { pragma, source })
}

fn verify_value(
    pragma: &'static str,
    expected: &'static str,
    actual: String,
) -> Result<(), DatabaseError> {
    if actual == expected {
        Ok(())
    } else {
        Err(DatabaseError::VerifyPragma {
            pragma,
            expected: expected.to_owned(),
            actual,
        })
    }
}
