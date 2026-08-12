use crate::error::PortFailure;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProviderKind {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderCapabilityStatus {
    Supported,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderCapabilities {
    pub claude: ProviderCapabilityStatus,
    pub codex: ProviderCapabilityStatus,
}

/// Minimal service boundary a future provider adapter implements to report
/// capability only. Session execution, streaming, and cancellation are
/// separate future ports so this boundary never requires them.
pub trait ProviderCapabilityPort {
    fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure>;
}
