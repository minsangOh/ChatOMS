use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, PlatformCapabilityStatus, StorageBootstrapPort,
    StorageBootstrapState, TimeProvider,
    error::{CategorizedFailure, FailureCategory, PortFailure},
    path::{AppPathResolver, ResolvedAppPaths},
    permissions::FilesystemPermissionManager,
};

use crate::SecureAppPaths;

pub type SharedResolvedAppPaths = Arc<Mutex<Option<ResolvedAppPaths>>>;

pub struct StorageBootstrapAdapter<R, P> {
    resolver: R,
    permissions: P,
    resolved_paths: SharedResolvedAppPaths,
}

impl<R, P> StorageBootstrapAdapter<R, P> {
    #[must_use]
    pub const fn new(resolver: R, permissions: P, resolved_paths: SharedResolvedAppPaths) -> Self {
        Self {
            resolver,
            permissions,
            resolved_paths,
        }
    }
}

#[cfg(windows)]
impl
    StorageBootstrapAdapter<
        crate::path::WindowsPathResolver,
        crate::permissions::WindowsPermissionManager,
    >
{
    pub fn windows_from_environment(
        resolved_paths: SharedResolvedAppPaths,
    ) -> Result<Self, PortFailure> {
        let resolver = crate::path::WindowsPathResolver::from_environment()
            .map_err(|error| port_failure(&error))?;
        Ok(Self::new(
            resolver,
            crate::permissions::WindowsPermissionManager,
            resolved_paths,
        ))
    }
}

impl<R, P> StorageBootstrapPort for StorageBootstrapAdapter<R, P>
where
    R: AppPathResolver,
    P: FilesystemPermissionManager,
{
    fn prepare_secure_storage(&mut self) -> Result<StorageBootstrapState, PortFailure> {
        let paths = SecureAppPaths::prepare(&self.resolver, &self.permissions)
            .map_err(|error| port_failure(&error))?;
        let mut stored = self
            .resolved_paths
            .lock()
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        *stored = Some(paths);
        Ok(StorageBootstrapState::Ready)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTimeProvider;

impl TimeProvider for SystemTimeProvider {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        system_time_to_unix_epoch_ms(SystemTime::now())
    }
}

#[doc(hidden)]
pub fn system_time_to_unix_epoch_ms(time: SystemTime) -> Result<i64, PortFailure> {
    let milliseconds = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PortFailure::new(FailureCategory::Internal))?
        .as_millis();
    i64::try_from(milliseconds).map_err(|_| PortFailure::new(FailureCategory::Internal))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StaticPlatformCapabilityAdapter;

impl PlatformCapabilityPort for StaticPlatformCapabilityAdapter {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        #[cfg(windows)]
        let status = PlatformCapabilityStatus::Supported;
        #[cfg(not(windows))]
        let status = PlatformCapabilityStatus::Unsupported;
        Ok(PlatformCapabilities {
            secure_storage: status,
            native_permissions: status,
        })
    }
}

fn port_failure(error: &impl CategorizedFailure) -> PortFailure {
    PortFailure::with_policy(error.category(), error.severity(), error.retry())
}
