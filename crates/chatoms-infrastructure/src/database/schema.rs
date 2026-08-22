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
pub(crate) const IMPLEMENTATION_CONSENTS_SQL: &str =
    include_str!("../../migrations/0008_implementation_consents.sql");
pub(crate) const TASK_IMPLEMENTATION_RESULTS_SQL: &str =
    include_str!("../../migrations/0009_task_implementation_results.sql");
pub(crate) const TASK_VALIDATION_COMMAND_APPROVALS_SQL: &str =
    include_str!("../../migrations/0010_task_validation_command_approvals.sql");
pub(crate) const VALIDATION_COMMAND_EXECUTABLE_BINDING_SQL: &str =
    include_str!("../../migrations/0011_validation_command_executable_binding.sql");
pub(crate) const VALIDATION_COMMAND_ENVIRONMENT_BINDING_SQL: &str =
    include_str!("../../migrations/0012_validation_command_environment_binding.sql");
pub(crate) const TASK_VALIDATION_COMMAND_RESULTS_SQL: &str =
    include_str!("../../migrations/0013_task_validation_command_results.sql");
pub(crate) const REVIEW_CONSENTS_SQL: &str =
    include_str!("../../migrations/0014_review_consents.sql");
pub(crate) const TASK_REVIEW_RESULTS_SQL: &str =
    include_str!("../../migrations/0015_task_review_results.sql");
pub(crate) const PROVIDER_CONSENT_DATA_SCOPE_SQL: &str =
    include_str!("../../migrations/0016_provider_consent_data_scope.sql");
pub(crate) const CONTEXT_PACKAGE_MANIFESTS_SQL: &str =
    include_str!("../../migrations/0017_context_package_manifests.sql");
pub(crate) const TASK_HIGH_RISK_APPROVALS_SQL: &str =
    include_str!("../../migrations/0018_task_high_risk_approvals.sql");
pub(crate) const TASK_DIFF_APPROVALS_SQL: &str =
    include_str!("../../migrations/0019_task_diff_approvals.sql");
pub(crate) const SCOPED_POST_MERGE_VALIDATION_SQL: &str =
    include_str!("../../migrations/0020_scoped_post_merge_validation.sql");
pub(crate) const MANUAL_MERGE_RESOLUTION_CONFIRMATIONS_SQL: &str =
    include_str!("../../migrations/0021_manual_merge_resolution_confirmations.sql");
pub(crate) const TASK_MERGE_ABORT_APPROVALS_SQL: &str =
    include_str!("../../migrations/0022_task_merge_abort_approvals.sql");

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
