/// The fixed vocabulary of high-risk design categories from
/// `PRODUCT_REQUIREMENTS.md` section 9 ("설계 승인 정책"), reused here as the
/// closed set of *effect* categories a future operation on a target project
/// may fall into. This type is provider-neutral, work-kind-neutral, and
/// task-state-neutral: it carries no information about which task, provider,
/// or operation a given category applies to, and no free-text description of
/// what specifically triggered it — see
/// `chatoms_ports::repository::HighRiskApprovalRecord` for the identity that
/// binds a category to a task and version.
///
/// All 13 categories are included even though several are not reachable by
/// any operation the current Claude execution contracts can perform (for
/// example `ExternalNetworkBehaviorAddition` or
/// `OperatingSystemConfigurationChange` — Claude Planning/Implementation/
/// Review all run under a fixed `--tools` allowlist and an `env_clear`'d
/// environment that structurally rules these out today). The vocabulary
/// itself is a closed enumeration of what *could* be classified as high-risk
/// in principle, independent of which categories any current or future
/// classifier actually reaches.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HighRiskCategory {
    ArchitectureChange,
    DatabaseSchemaChange,
    AuthenticationOrAuthorizationChange,
    SecurityPolicyChange,
    ExternalNetworkBehaviorAddition,
    ExternalDataTransmissionAddition,
    LargeScaleFileMoveOrDeletion,
    PublicApiOrStorageFormatChange,
    OperatingSystemConfigurationChange,
    AdministratorPrivilegesRequired,
    BreakingCompatibilityChange,
    DataMigration,
    DifficultToRecoverChange,
}

impl HighRiskCategory {
    pub const ALL: [Self; 13] = [
        Self::ArchitectureChange,
        Self::DatabaseSchemaChange,
        Self::AuthenticationOrAuthorizationChange,
        Self::SecurityPolicyChange,
        Self::ExternalNetworkBehaviorAddition,
        Self::ExternalDataTransmissionAddition,
        Self::LargeScaleFileMoveOrDeletion,
        Self::PublicApiOrStorageFormatChange,
        Self::OperatingSystemConfigurationChange,
        Self::AdministratorPrivilegesRequired,
        Self::BreakingCompatibilityChange,
        Self::DataMigration,
        Self::DifficultToRecoverChange,
    ];

    /// The exact persisted text this category maps to in
    /// `task_high_risk_approvals.risk_category` (and its SQL `CHECK`
    /// constraint) — the single source of truth both this method and
    /// [`Self::from_persisted_text`] agree on.
    #[must_use]
    pub const fn persisted_text(self) -> &'static str {
        match self {
            Self::ArchitectureChange => "ArchitectureChange",
            Self::DatabaseSchemaChange => "DatabaseSchemaChange",
            Self::AuthenticationOrAuthorizationChange => "AuthenticationOrAuthorizationChange",
            Self::SecurityPolicyChange => "SecurityPolicyChange",
            Self::ExternalNetworkBehaviorAddition => "ExternalNetworkBehaviorAddition",
            Self::ExternalDataTransmissionAddition => "ExternalDataTransmissionAddition",
            Self::LargeScaleFileMoveOrDeletion => "LargeScaleFileMoveOrDeletion",
            Self::PublicApiOrStorageFormatChange => "PublicApiOrStorageFormatChange",
            Self::OperatingSystemConfigurationChange => "OperatingSystemConfigurationChange",
            Self::AdministratorPrivilegesRequired => "AdministratorPrivilegesRequired",
            Self::BreakingCompatibilityChange => "BreakingCompatibilityChange",
            Self::DataMigration => "DataMigration",
            Self::DifficultToRecoverChange => "DifficultToRecoverChange",
        }
    }

    /// Parses a persisted value back into a category. An unrecognized value
    /// (a corrupted or hand-edited database row) returns `None` rather than
    /// a default or fallback category — callers must fail closed on `None`,
    /// never substitute a guessed category.
    #[must_use]
    pub fn from_persisted_text(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.persisted_text() == value)
    }
}
