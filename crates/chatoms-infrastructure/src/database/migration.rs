use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use super::{DatabaseConnection, DatabaseError, schema};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

impl Migration {
    #[must_use]
    pub const fn new(version: u32, name: &'static str, sql: &'static str) -> Self {
        Self { version, name, sql }
    }

    #[must_use]
    pub fn checksum_sha256(self) -> String {
        checksum_sha256(self.sql.as_bytes())
    }
}

pub static FOUNDATION_MIGRATION: [Migration; 1] =
    [Migration::new(1, "foundation", schema::FOUNDATION_SQL)];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationOutcome {
    pub schema_version: u32,
    pub applied_count: usize,
}

pub struct MigrationRunner {
    registry: &'static [Migration],
}

impl Default for MigrationRunner {
    fn default() -> Self {
        Self::new(&FOUNDATION_MIGRATION)
    }
}

impl MigrationRunner {
    #[must_use]
    pub const fn new(registry: &'static [Migration]) -> Self {
        Self { registry }
    }

    pub fn run(
        &self,
        database: &mut DatabaseConnection,
    ) -> Result<MigrationOutcome, DatabaseError> {
        bootstrap_metadata(database.raw_mut())?;
        validate_registry(self.registry)?;

        let applied = load_applied(database.raw_mut())?;
        let application_version = self
            .registry
            .last()
            .map_or(0, |migration| migration.version);
        if let Some(database_version) = applied.last().map(|migration| migration.version)
            && database_version > application_version
        {
            return Err(DatabaseError::DatabaseNewerThanApplication {
                database_version,
                application_version,
            });
        }

        validate_applied_prefix(self.registry, &applied)?;

        let mut applied_count = 0;
        for migration in self.registry.iter().skip(applied.len()).copied() {
            apply_one(database.raw_mut(), migration)?;
            applied_count += 1;
        }

        Ok(MigrationOutcome {
            schema_version: self
                .registry
                .last()
                .map_or(0, |migration| migration.version),
            applied_count,
        })
    }
}

#[must_use]
pub fn checksum_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut checksum = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut checksum, "{byte:02x}").expect("writing to String cannot fail");
    }
    checksum
}

pub fn validate_registry(registry: &[Migration]) -> Result<(), DatabaseError> {
    if registry.is_empty() {
        return Err(DatabaseError::MigrationRegistryInvalid {
            reason: "migration registry must start at version one",
        });
    }
    let mut previous = 0;
    for (index, migration) in registry.iter().enumerate() {
        if migration.version == 0 {
            return Err(DatabaseError::MigrationRegistryInvalid {
                reason: "migration version zero is forbidden",
            });
        }
        if index == 0 && migration.version != 1 {
            return Err(DatabaseError::MigrationRegistryInvalid {
                reason: "migration registry must start at version one",
            });
        }
        if migration.version <= previous {
            return Err(DatabaseError::MigrationRegistryInvalid {
                reason: "migration versions must be strictly increasing",
            });
        }
        if migration.name.is_empty() {
            return Err(DatabaseError::MigrationRegistryInvalid {
                reason: "migration name must not be empty",
            });
        }
        if migration.sql.is_empty() {
            return Err(DatabaseError::MigrationRegistryInvalid {
                reason: "migration SQL must not be empty",
            });
        }
        let checksum = migration.checksum_sha256();
        if !is_lowercase_sha256(&checksum) {
            return Err(DatabaseError::MigrationRegistryInvalid {
                reason: "migration checksum must be lowercase SHA-256 hex",
            });
        }
        previous = migration.version;
    }
    Ok(())
}

#[derive(Debug)]
struct AppliedMigration {
    version: u32,
    name: String,
    checksum: String,
}

fn bootstrap_metadata(connection: &mut Connection) -> Result<(), DatabaseError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| DatabaseError::MigrationExecutionFailed {
            version: 0,
            name: "schema_migrations",
            source,
        })?;
    transaction
        .execute_batch(schema::METADATA_TABLE_SQL)
        .map_err(|source| DatabaseError::MigrationExecutionFailed {
            version: 0,
            name: "schema_migrations",
            source,
        })?;
    validate_metadata_table(&transaction)?;
    transaction
        .commit()
        .map_err(|source| DatabaseError::MigrationExecutionFailed {
            version: 0,
            name: "schema_migrations",
            source,
        })
}

fn validate_metadata_table(connection: &Connection) -> Result<(), DatabaseError> {
    let mut statement = connection
        .prepare("PRAGMA table_info(schema_migrations)")
        .map_err(|source| DatabaseError::MigrationMetadataInvalid {
            reason: "schema_migrations columns could not be inspected",
            source: Some(source),
        })?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|source| DatabaseError::MigrationMetadataInvalid {
            reason: "schema_migrations columns could not be read",
            source: Some(source),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::MigrationMetadataInvalid {
            reason: "schema_migrations columns are malformed",
            source: Some(source),
        })?;

    let expected = [
        ("version", "INTEGER", 0, 1),
        ("name", "TEXT", 1, 0),
        ("checksum_sha256", "TEXT", 1, 0),
        ("applied_at_ms", "INTEGER", 1, 0),
    ];
    if columns.len() != expected.len()
        || columns.iter().zip(expected).any(
            |((name, data_type, not_null, primary_key), expected)| {
                name != expected.0
                    || !data_type.eq_ignore_ascii_case(expected.1)
                    || *not_null != expected.2
                    || *primary_key != expected.3
            },
        )
    {
        return Err(DatabaseError::MigrationMetadataInvalid {
            reason: "schema_migrations has an unexpected structure",
            source: None,
        });
    }

    let table_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .map_err(|source| DatabaseError::MigrationMetadataInvalid {
            reason: "schema_migrations definition could not be read",
            source: Some(source),
        })?;
    let normalized_sql = table_sql
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    for required_constraint in [
        "check(version>=1)",
        "check(length(name)>0)",
        "check(length(checksum_sha256)=64andchecksum_sha256notglob'*[^0-9a-f]*')",
        "check(applied_at_ms>=0)",
    ] {
        if !normalized_sql.contains(required_constraint) {
            return Err(DatabaseError::MigrationMetadataInvalid {
                reason: "schema_migrations constraints do not match policy",
                source: None,
            });
        }
    }
    Ok(())
}

fn load_applied(connection: &Connection) -> Result<Vec<AppliedMigration>, DatabaseError> {
    validate_metadata_table(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT version, name, checksum_sha256
             FROM schema_migrations
             ORDER BY version",
        )
        .map_err(|source| DatabaseError::MigrationMetadataInvalid {
            reason: "applied migrations could not be queried",
            source: Some(source),
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|source| DatabaseError::MigrationMetadataInvalid {
            reason: "applied migrations could not be read",
            source: Some(source),
        })?;

    let mut applied = Vec::new();
    for row in rows {
        let (version, name, checksum) =
            row.map_err(|source| DatabaseError::MigrationMetadataInvalid {
                reason: "applied migration row is malformed",
                source: Some(source),
            })?;
        let version =
            u32::try_from(version).map_err(|_| DatabaseError::MigrationMetadataInvalid {
                reason: "applied migration version is outside the supported range",
                source: None,
            })?;
        if version == 0 || name.is_empty() || !is_lowercase_sha256(&checksum) {
            return Err(DatabaseError::MigrationMetadataInvalid {
                reason: "applied migration values violate metadata policy",
                source: None,
            });
        }
        applied.push(AppliedMigration {
            version,
            name,
            checksum,
        });
    }
    Ok(applied)
}

fn validate_applied_prefix(
    registry: &[Migration],
    applied: &[AppliedMigration],
) -> Result<(), DatabaseError> {
    for (index, existing) in applied.iter().enumerate() {
        let expected = registry
            .get(index)
            .ok_or(DatabaseError::DatabaseNewerThanApplication {
                database_version: existing.version,
                application_version: registry.last().map_or(0, |migration| migration.version),
            })?;
        if existing.version != expected.version {
            return Err(DatabaseError::MigrationOutOfOrder {
                expected: expected.version,
                found: existing.version,
            });
        }
        if existing.name != expected.name {
            return Err(DatabaseError::MigrationMetadataInvalid {
                reason: "applied migration name differs from the registry",
                source: None,
            });
        }
        if existing.checksum != expected.checksum_sha256() {
            return Err(DatabaseError::MigrationChecksumMismatch {
                version: existing.version,
            });
        }
    }
    Ok(())
}

fn apply_one(connection: &mut Connection, migration: Migration) -> Result<(), DatabaseError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| migration_error(migration, source))?;
    transaction
        .execute_batch(migration.sql)
        .map_err(|source| migration_error(migration, source))?;

    let violations = foreign_key_violation_count(&transaction, migration)?;
    if violations != 0 {
        return Err(DatabaseError::ForeignKeyViolation {
            version: migration.version,
            violations,
        });
    }

    let applied_at_ms = current_unix_epoch_ms()?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum_sha256, applied_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(migration.version),
                migration.name,
                migration.checksum_sha256(),
                applied_at_ms
            ],
        )
        .map_err(|source| migration_error(migration, source))?;
    transaction
        .commit()
        .map_err(|source| migration_error(migration, source))
}

fn foreign_key_violation_count(
    connection: &Connection,
    migration: Migration,
) -> Result<usize, DatabaseError> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|source| migration_error(migration, source))?;
    let mut rows = statement
        .query([])
        .map_err(|source| migration_error(migration, source))?;
    let mut count = 0;
    while rows
        .next()
        .map_err(|source| migration_error(migration, source))?
        .is_some()
    {
        count += 1;
    }
    Ok(count)
}

fn current_unix_epoch_ms() -> Result<i64, DatabaseError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        DatabaseError::InvariantViolation {
            reason: "system time is before Unix epoch",
        }
    })?;
    i64::try_from(duration.as_millis()).map_err(|_| DatabaseError::InvariantViolation {
        reason: "system time exceeds supported timestamp range",
    })
}

fn migration_error(migration: Migration, source: rusqlite::Error) -> DatabaseError {
    DatabaseError::MigrationExecutionFailed {
        version: migration.version,
        name: migration.name,
        source,
    }
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
