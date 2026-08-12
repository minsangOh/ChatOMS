use chatoms_application::{
    APPLICATION_VERSION,
    bootstrap::{ActiveTaskStatus, BootstrapStatus, DatabaseStatus, LoggingStatus, StorageStatus},
    error::ApplicationErrorCode,
    system::{CapabilityStatus, HealthStatus, SystemService},
};
use chatoms_domain::TaskId;
use chatoms_ports::{
    PlatformCapabilities, PlatformCapabilityPort, PlatformCapabilityStatus,
    error::{FailureCategory, PortFailure},
};

struct CapabilityFake(Result<PlatformCapabilities, PortFailure>);

impl PlatformCapabilityPort for CapabilityFake {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure> {
        self.0
    }
}

fn capabilities(status: PlatformCapabilityStatus) -> PlatformCapabilities {
    PlatformCapabilities {
        secure_storage: status,
        native_permissions: status,
    }
}

fn bootstrap(
    storage_status: StorageStatus,
    database_status: DatabaseStatus,
    logging_status: LoggingStatus,
) -> BootstrapStatus {
    BootstrapStatus {
        storage_status,
        database_status,
        logging_status,
        active_task_status: ActiveTaskStatus::Active {
            task_id: TaskId::new(),
            acquired_at_ms: 12,
        },
        application_version: APPLICATION_VERSION,
        ready: storage_status == StorageStatus::Ready && database_status.is_ready(),
    }
}

#[test]
fn healthy_status_preserves_version_readiness_and_active_task() {
    let bootstrap = bootstrap(
        StorageStatus::Ready,
        DatabaseStatus::Ready,
        LoggingStatus::Ready,
    );
    let mut capability = CapabilityFake(Ok(capabilities(PlatformCapabilityStatus::Supported)));
    let mut service = SystemService::new(&bootstrap, &mut capability);
    assert_eq!(service.get_version(), APPLICATION_VERSION);
    let status = service.get_system_status().expect("system status");
    assert_eq!(status.health, HealthStatus::Healthy);
    assert_eq!(status.storage_status, StorageStatus::Ready);
    assert_eq!(status.database_status, DatabaseStatus::Ready);
    assert_eq!(status.logging_status, LoggingStatus::Ready);
    assert!(matches!(
        status.active_task_status,
        ActiveTaskStatus::Active { .. }
    ));
    assert_eq!(
        status.capabilities.secure_storage,
        CapabilityStatus::Supported
    );
}

#[test]
fn logging_or_noncritical_capability_unavailable_is_degraded() {
    let cases = [
        (
            LoggingStatus::Unavailable,
            PlatformCapabilityStatus::Supported,
        ),
        (LoggingStatus::Ready, PlatformCapabilityStatus::Unsupported),
    ];
    for (logging, capability_status) in cases {
        let bootstrap = bootstrap(StorageStatus::Ready, DatabaseStatus::Ready, logging);
        let mut capability = CapabilityFake(Ok(capabilities(capability_status)));
        let mut service = SystemService::new(&bootstrap, &mut capability);
        assert_eq!(
            service.get_health().expect("health"),
            HealthStatus::Degraded
        );
    }
}

#[test]
fn insecure_storage_or_unavailable_database_is_unavailable() {
    for (storage, database) in [
        (StorageStatus::Insecure, DatabaseStatus::NotChecked),
        (StorageStatus::Ready, DatabaseStatus::Unavailable),
        (StorageStatus::Ready, DatabaseStatus::Incompatible),
    ] {
        let bootstrap = bootstrap(storage, database, LoggingStatus::NotChecked);
        let mut capability = CapabilityFake(Ok(capabilities(PlatformCapabilityStatus::Supported)));
        let mut service = SystemService::new(&bootstrap, &mut capability);
        assert_eq!(
            service.get_health().expect("health"),
            HealthStatus::Unavailable
        );
    }
}

#[test]
fn provider_capability_placeholder_is_fail_closed_and_does_not_affect_health() {
    let bootstrap = bootstrap(
        StorageStatus::Ready,
        DatabaseStatus::Ready,
        LoggingStatus::Ready,
    );
    let mut capability = CapabilityFake(Ok(capabilities(PlatformCapabilityStatus::Supported)));
    let mut service = SystemService::new(&bootstrap, &mut capability);
    let status = service.get_system_status().expect("system status");
    assert_eq!(status.health, HealthStatus::Healthy);
    assert_eq!(status.provider_capabilities.claude, None);
    assert_eq!(status.provider_capabilities.codex, None);
}

#[test]
fn capability_failure_maps_to_safe_application_error() {
    let bootstrap = bootstrap(
        StorageStatus::Ready,
        DatabaseStatus::Ready,
        LoggingStatus::Ready,
    );
    let mut capability = CapabilityFake(Err(PortFailure::new(FailureCategory::Unsupported)));
    let mut service = SystemService::new(&bootstrap, &mut capability);
    let error = service.get_system_status().expect_err("capability error");
    assert_eq!(error.code(), ApplicationErrorCode::Unsupported);
    assert_eq!(
        error.to_string(),
        "APP_UNSUPPORTED: This operation is not supported on the current platform."
    );
}
