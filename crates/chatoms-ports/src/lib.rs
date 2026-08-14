#![doc = "Port boundary for persistence, platform, provider, Git, process, and update interfaces."]
#![forbid(unsafe_code)]

pub mod diff;
pub mod error;
pub mod filesystem;
pub mod git;
pub mod implementation;
pub mod path;
pub mod permissions;
pub mod planning;
pub mod process;
pub mod provider;
pub mod repository;
pub mod review;
pub mod validation;
pub mod validation_execution;

use error::PortFailure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageBootstrapState {
    Ready,
    Unavailable,
    Insecure,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseBootstrapState {
    Ready,
    Upgraded,
    MigrationRequired,
    Unavailable,
    Incompatible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoggingBootstrapState {
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformCapabilityStatus {
    Supported,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformCapabilities {
    pub secure_storage: PlatformCapabilityStatus,
    pub native_permissions: PlatformCapabilityStatus,
}

pub trait StorageBootstrapPort {
    fn prepare_secure_storage(&mut self) -> Result<StorageBootstrapState, PortFailure>;
}

pub trait DatabaseBootstrapPort {
    fn bootstrap_database(&mut self) -> Result<DatabaseBootstrapState, PortFailure>;
}

pub trait LoggingBootstrapPort {
    fn bootstrap_logging(&mut self) -> Result<LoggingBootstrapState, PortFailure>;
}

pub trait PlatformCapabilityPort {
    fn platform_capabilities(&mut self) -> Result<PlatformCapabilities, PortFailure>;
}

pub trait TimeProvider {
    fn now_ms(&mut self) -> Result<i64, PortFailure>;
}
