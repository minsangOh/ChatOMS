#![doc = "Application boundary for ChatOMS use cases and transaction orchestration."]
#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod context_package_implementation_execution;
pub mod context_package_planning_execution;
pub mod context_package_review_execution;
pub mod error;
pub mod git_isolation;
pub mod implementation_execution;
pub mod merge_execution;
pub mod planning_execution;
pub mod post_merge_validation;
pub mod projects;
pub mod provider;
pub mod provider_eligibility;
pub mod review_diff;
pub mod review_execution;
pub mod system;
pub mod tasks;
pub mod testing_execution;
pub mod user_diff_approval;
pub mod validation_commands;

pub const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");
