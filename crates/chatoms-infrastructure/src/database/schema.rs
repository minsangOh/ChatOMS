pub(crate) const FOUNDATION_SQL: &str = include_str!("../../migrations/0001_foundation.sql");
pub(crate) const GIT_ISOLATION_SQL: &str = include_str!("../../migrations/0002_git_isolation.sql");
pub(crate) const PROVIDER_BINDING_SQL: &str =
    include_str!("../../migrations/0003_provider_binding.sql");
pub(crate) const PROVIDER_NEUTRAL_TASK_STATES_SQL: &str =
    include_str!("../../migrations/0004_provider_neutral_task_states.sql");
pub(crate) const TASK_BRIEFS_SQL: &str = include_str!("../../migrations/0005_task_briefs.sql");
pub(crate) const PROVIDER_CONSENTS_SQL: &str =
    include_str!("../../migrations/0006_provider_consents.sql");
pub(crate) const TASK_PLANNING_RESULTS_SQL: &str =
    include_str!("../../migrations/0007_task_planning_results.sql");

pub(crate) const METADATA_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version >= 1),
    name TEXT NOT NULL CHECK (length(name) > 0),
    checksum_sha256 TEXT NOT NULL CHECK (
        length(checksum_sha256) = 64
        AND checksum_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
);
"#;
