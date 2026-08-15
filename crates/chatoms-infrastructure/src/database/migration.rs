use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use super::{DatabaseConnection, DatabaseError, schema};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyProject {
    pub project_id: String,
    pub name: String,
    pub root_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyProjectIdentity {
    pub project_id: String,
    pub canonical_path_key: String,
    pub display_path: String,
    pub root_volume_serial_hex: String,
    pub root_file_id_hex: String,
    pub repository_kind: &'static str,
    pub git_common_volume_serial_hex: Option<String>,
    pub git_common_file_id_hex: Option<String>,
}

pub trait LegacyProjectPreflight {
    fn resolve(
        &mut self,
        projects: &[LegacyProject],
    ) -> Result<Vec<LegacyProjectIdentity>, DatabaseError>;
}

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

pub static FOUNDATION_MIGRATION: [Migration; 19] = [
    Migration::new(1, "foundation", schema::FOUNDATION_SQL),
    Migration::new(2, "git_isolation", schema::GIT_ISOLATION_SQL),
    Migration::new(3, "provider_binding", schema::PROVIDER_BINDING_SQL),
    Migration::new(
        4,
        "provider_neutral_task_states",
        schema::PROVIDER_NEUTRAL_TASK_STATES_SQL,
    ),
    Migration::new(5, "task_briefs", schema::TASK_BRIEFS_SQL),
    Migration::new(6, "provider_consents", schema::PROVIDER_CONSENTS_SQL),
    Migration::new(
        7,
        "task_planning_results",
        schema::TASK_PLANNING_RESULTS_SQL,
    ),
    Migration::new(
        8,
        "implementation_consents",
        schema::IMPLEMENTATION_CONSENTS_SQL,
    ),
    Migration::new(
        9,
        "task_implementation_results",
        schema::TASK_IMPLEMENTATION_RESULTS_SQL,
    ),
    Migration::new(
        10,
        "task_validation_command_approvals",
        schema::TASK_VALIDATION_COMMAND_APPROVALS_SQL,
    ),
    Migration::new(
        11,
        "validation_command_executable_binding",
        schema::VALIDATION_COMMAND_EXECUTABLE_BINDING_SQL,
    ),
    Migration::new(
        12,
        "validation_command_environment_binding",
        schema::VALIDATION_COMMAND_ENVIRONMENT_BINDING_SQL,
    ),
    Migration::new(
        13,
        "task_validation_command_results",
        schema::TASK_VALIDATION_COMMAND_RESULTS_SQL,
    ),
    Migration::new(14, "review_consents", schema::REVIEW_CONSENTS_SQL),
    Migration::new(15, "task_review_results", schema::TASK_REVIEW_RESULTS_SQL),
    Migration::new(
        16,
        "provider_consent_data_scope",
        schema::PROVIDER_CONSENT_DATA_SCOPE_SQL,
    ),
    Migration::new(
        17,
        "context_package_manifests",
        schema::CONTEXT_PACKAGE_MANIFESTS_SQL,
    ),
    Migration::new(
        18,
        "task_high_risk_approvals",
        schema::TASK_HIGH_RISK_APPROVALS_SQL,
    ),
    Migration::new(19, "task_diff_approvals", schema::TASK_DIFF_APPROVALS_SQL),
];

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
        self.run_internal(database, None)
    }

    pub fn run_with_preflight(
        &self,
        database: &mut DatabaseConnection,
        preflight: &mut dyn LegacyProjectPreflight,
    ) -> Result<MigrationOutcome, DatabaseError> {
        self.run_internal(database, Some(preflight))
    }

    fn run_internal(
        &self,
        database: &mut DatabaseConnection,
        mut preflight: Option<&mut dyn LegacyProjectPreflight>,
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
            let identities = if migration.version == 2 && migration.name == "git_isolation" {
                let projects = load_legacy_projects(database.raw_mut())?;
                if projects.is_empty() {
                    Vec::new()
                } else if let Some(resolver) = preflight.as_deref_mut() {
                    validate_preflight_result(&projects, resolver.resolve(&projects)?)?
                } else {
                    return Err(DatabaseError::LegacyProjectPreflightFailed {
                        project_id: projects[0].project_id.clone(),
                        display_path: safe_legacy_display(&projects[0]),
                        reason: "stable filesystem identity was not confirmed",
                    });
                }
            } else {
                Vec::new()
            };
            apply_one(database.raw_mut(), migration, &identities)?;
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

fn load_legacy_projects(connection: &Connection) -> Result<Vec<LegacyProject>, DatabaseError> {
    let mut statement = connection
        .prepare("SELECT id, name, root_path FROM projects ORDER BY id")
        .map_err(|source| DatabaseError::MigrationExecutionFailed {
            version: 2,
            name: "git_isolation",
            source,
        })?;
    statement
        .query_map([], |row| {
            Ok(LegacyProject {
                project_id: row.get(0)?,
                name: row.get(1)?,
                root_path: row.get(2)?,
            })
        })
        .map_err(|source| DatabaseError::MigrationExecutionFailed {
            version: 2,
            name: "git_isolation",
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| DatabaseError::MigrationExecutionFailed {
            version: 2,
            name: "git_isolation",
            source,
        })
}

fn validate_preflight_result(
    projects: &[LegacyProject],
    identities: Vec<LegacyProjectIdentity>,
) -> Result<Vec<LegacyProjectIdentity>, DatabaseError> {
    if identities.len() != projects.len() {
        return Err(DatabaseError::LegacyProjectPreflightFailed {
            project_id: projects[0].project_id.clone(),
            display_path: safe_legacy_display(&projects[0]),
            reason: "preflight did not resolve every legacy project",
        });
    }
    let mut project_ids = std::collections::HashSet::new();
    let mut canonical_keys = std::collections::HashSet::new();
    let mut stable_ids = std::collections::HashSet::new();
    for identity in &identities {
        if !project_ids.insert(identity.project_id.clone())
            || !projects
                .iter()
                .any(|project| project.project_id == identity.project_id)
            || !canonical_keys.insert(identity.canonical_path_key.clone())
            || !stable_ids.insert((
                identity.root_volume_serial_hex.clone(),
                identity.root_file_id_hex.clone(),
            ))
        {
            return Err(DatabaseError::LegacyProjectPreflightFailed {
                project_id: identity.project_id.clone(),
                display_path: identity.display_path.clone(),
                reason: "duplicate or ambiguous stable project identity",
            });
        }
    }
    Ok(identities)
}

fn safe_legacy_display(project: &LegacyProject) -> String {
    let tail = std::path::Path::new(&project.root_path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter(|component| !component.is_empty())
        .rev()
        .take(2)
        .collect::<Vec<_>>();
    if tail.is_empty() {
        project.name.clone()
    } else {
        format!(
            "…\\{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\\")
        )
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

fn validate_no_existing_approvals_then_apply(
    connection: &mut Connection,
    migration: Migration,
    identities: &[LegacyProjectIdentity],
) -> Result<(), DatabaseError> {
    let has_existing_approval: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_validation_command_approvals LIMIT 1)",
            [],
            |row| row.get(0),
        )
        .map_err(|source| migration_error(migration, source))?;

    if has_existing_approval {
        return Err(DatabaseError::ValidationCommandApprovalMigrationFailed {
            reason: "existing approval records must be removed before applying this migration",
        });
    }

    apply_one_transaction(connection, migration, identities)
}

/// Unlike 0011 (whose pre-existing rows were always dev/test scratch data,
/// since no shipped UI had ever written one), this table may already hold
/// real user-approved rows by the time migration 12 runs — this Unit's own
/// approval-persisting code shipped with 0011. Fabricating environment
/// identity for such a row would misrepresent what was actually verified,
/// so migration 12 aborts before touching the table if any row already
/// exists, exactly mirroring 0011's own precondition.
fn validate_no_existing_approvals_then_apply_environment_binding(
    connection: &mut Connection,
    migration: Migration,
    identities: &[LegacyProjectIdentity],
) -> Result<(), DatabaseError> {
    let has_existing_approval: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_validation_command_approvals LIMIT 1)",
            [],
            |row| row.get(0),
        )
        .map_err(|source| migration_error(migration, source))?;

    if has_existing_approval {
        return Err(
            DatabaseError::ValidationCommandEnvironmentBindingMigrationFailed {
                reason: "existing approval records must be removed before applying this migration",
            },
        );
    }

    apply_one_transaction(connection, migration, identities)
}

fn apply_one(
    connection: &mut Connection,
    migration: Migration,
    identities: &[LegacyProjectIdentity],
) -> Result<(), DatabaseError> {
    if migration.version == 12 && migration.name == "validation_command_environment_binding" {
        return validate_no_existing_approvals_then_apply_environment_binding(
            connection, migration, identities,
        );
    }
    if migration.version == 11 && migration.name == "validation_command_executable_binding" {
        return validate_no_existing_approvals_then_apply(connection, migration, identities);
    }

    if migration.version != 4 || migration.name != "provider_neutral_task_states" {
        return apply_one_transaction(connection, migration, identities);
    }

    set_foreign_keys(connection, false)?;
    let migration_result = apply_one_transaction(connection, migration, identities);
    let restore_result = set_foreign_keys(connection, true);
    match restore_result {
        Ok(()) => migration_result,
        Err(error) => Err(error),
    }
}

fn set_foreign_keys(connection: &Connection, enabled: bool) -> Result<(), DatabaseError> {
    connection
        .pragma_update(None, "foreign_keys", enabled)
        .map_err(|source| DatabaseError::ConfigureDatabase {
            pragma: "foreign_keys",
            source,
        })?;
    let actual: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|source| DatabaseError::ConfigureDatabase {
            pragma: "foreign_keys",
            source,
        })?;
    let expected = if enabled { 1 } else { 0 };
    if actual == expected {
        Ok(())
    } else {
        Err(DatabaseError::VerifyPragma {
            pragma: "foreign_keys",
            expected: expected.to_string(),
            actual: actual.to_string(),
        })
    }
}

fn apply_one_transaction(
    connection: &mut Connection,
    migration: Migration,
    identities: &[LegacyProjectIdentity],
) -> Result<(), DatabaseError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| migration_error(migration, source))?;
    transaction
        .execute_batch(migration.sql)
        .map_err(|source| migration_error(migration, source))?;
    if migration.version == 2 && migration.name == "git_isolation" {
        for identity in identities {
            transaction
                .execute(
                    "UPDATE projects
                     SET canonical_path_key = ?2, display_path = ?3
                     WHERE id = ?1 AND canonical_path_key IS NULL AND display_path IS NULL",
                    params![
                        identity.project_id,
                        identity.canonical_path_key,
                        identity.display_path
                    ],
                )
                .map_err(|source| migration_error(migration, source))?;
            transaction
                .execute(
                    "INSERT INTO project_filesystem_identities (
                        project_id, identity_scheme, root_volume_serial_hex, root_file_id_hex,
                        repository_kind, git_common_volume_serial_hex, git_common_file_id_hex,
                        confirmed, revision, verified_at_ms
                     ) VALUES (?1, 'WindowsFileIdV1', ?2, ?3, ?4, ?5, ?6, 1, 1, ?7)",
                    params![
                        identity.project_id,
                        identity.root_volume_serial_hex,
                        identity.root_file_id_hex,
                        identity.repository_kind,
                        identity.git_common_volume_serial_hex,
                        identity.git_common_file_id_hex,
                        current_unix_epoch_ms()?
                    ],
                )
                .map_err(|source| migration_error(migration, source))?;
        }
        let unresolved: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM projects
                 WHERE canonical_path_key IS NULL OR display_path IS NULL
                    OR NOT EXISTS (
                        SELECT 1 FROM project_filesystem_identities AS identity
                        WHERE identity.project_id = projects.id AND identity.confirmed = 1
                    )",
                [],
                |row| row.get(0),
            )
            .map_err(|source| migration_error(migration, source))?;
        if unresolved != 0 {
            return Err(DatabaseError::LegacyProjectPreflightFailed {
                project_id: "multiple".to_owned(),
                display_path: "legacy projects".to_owned(),
                reason: "migration left an unconfirmed project identity",
            });
        }
    }

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
