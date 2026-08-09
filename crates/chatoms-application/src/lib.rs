#![doc = "Application boundary for ChatOMS use cases and transaction orchestration."]
#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod error;
pub mod git_isolation;
pub mod projects;
pub mod system;
pub mod tasks;

pub const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");
