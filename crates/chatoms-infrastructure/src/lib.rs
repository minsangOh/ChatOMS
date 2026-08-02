#![doc = "Infrastructure boundary for local persistence, migrations, redaction, and logging adapters."]
#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod database;
pub mod error;
pub mod logging;
pub mod redaction;
