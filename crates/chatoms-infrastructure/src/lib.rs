#![doc = "Infrastructure boundary for local persistence, migrations, redaction, and logging adapters."]
#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod claude_implementation;
pub mod claude_planning;
pub mod claude_review;
pub mod context_package;
pub mod database;
pub mod error;
pub mod git;
pub mod logging;
pub mod merge_execution;
pub mod process;
pub mod provider;
pub mod redaction;
pub mod validation_discovery;
pub mod validation_execution;
