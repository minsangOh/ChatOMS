use chatoms_ports::{PlatformCapabilities, PlatformCapabilityPort, PlatformCapabilityStatus};

use crate::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
    error::ApplicationError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilitySummary {
    pub secure_storage: CapabilityStatus,
    pub native_permissions: CapabilityStatus,
}

/// Provider execution capability, kept separate from `CapabilitySummary` so
/// platform and provider capability are never mixed. `None` means no
/// `ProviderCapabilityPort` adapter is wired yet; callers must treat that
/// the same as an unsupported capability rather than assuming success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderCapabilitySummary {
    pub claude: Option<CapabilityStatus>,
    pub codex: Option<CapabilityStatus>,
}

impl ProviderCapabilitySummary {
    #[must_use]
    pub const fn not_yet_probed() -> Self {
        Self {
            claude: None,
            codex: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemStatus {
    pub application_version: &'static str,
    pub health: HealthStatus,
    pub storage_status: StorageStatus,
    pub database_status: DatabaseStatus,
    pub logging_status: LoggingStatus,
    pub active_task_status: ActiveTaskStatus,
    pub capabilities: CapabilitySummary,
    pub provider_capabilities: ProviderCapabilitySummary,
}

pub struct SystemService<'a, C> {
    bootstrap: &'a BootstrapStatus,
    capabilities: &'a mut C,
}

impl<'a, C> SystemService<'a, C>
where
    C: PlatformCapabilityPort,
{
    #[must_use]
    pub fn new(bootstrap: &'a BootstrapStatus, capabilities: &'a mut C) -> Self {
        Self {
            bootstrap,
            capabilities,
        }
    }

    #[must_use]
    pub const fn get_version(&self) -> &'static str {
        APPLICATION_VERSION
    }

    pub fn get_health(&mut self) -> Result<HealthStatus, ApplicationError> {
        self.get_system_status().map(|status| status.health)
    }

    pub fn get_system_status(&mut self) -> Result<SystemStatus, ApplicationError> {
        let capabilities = self
            .capabilities
            .platform_capabilities()
            .map(CapabilitySummary::from)
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let health = classify_health(self.bootstrap, capabilities);
        Ok(SystemStatus {
            application_version: APPLICATION_VERSION,
            health,
            storage_status: self.bootstrap.storage_status,
            database_status: self.bootstrap.database_status,
            logging_status: self.bootstrap.logging_status,
            active_task_status: self.bootstrap.active_task_status,
            capabilities,
            provider_capabilities: ProviderCapabilitySummary::not_yet_probed(),
        })
    }
}

fn classify_health(status: &BootstrapStatus, capabilities: CapabilitySummary) -> HealthStatus {
    if status.storage_status != StorageStatus::Ready || !status.database_status.is_ready() {
        return HealthStatus::Unavailable;
    }
    if status.logging_status != LoggingStatus::Ready
        || capabilities.secure_storage == CapabilityStatus::Unsupported
        || capabilities.native_permissions == CapabilityStatus::Unsupported
    {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    }
}

impl From<PlatformCapabilities> for CapabilitySummary {
    fn from(value: PlatformCapabilities) -> Self {
        Self {
            secure_storage: CapabilityStatus::from(value.secure_storage),
            native_permissions: CapabilityStatus::from(value.native_permissions),
        }
    }
}

impl From<PlatformCapabilityStatus> for CapabilityStatus {
    fn from(value: PlatformCapabilityStatus) -> Self {
        match value {
            PlatformCapabilityStatus::Supported => Self::Supported,
            PlatformCapabilityStatus::Unsupported => Self::Unsupported,
        }
    }
}
