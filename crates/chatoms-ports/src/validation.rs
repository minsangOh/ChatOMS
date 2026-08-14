//! Port boundary for discovering candidate Testing validation commands
//! (format/lint/typecheck/test/build) from a task worktree's own manifest
//! files.
//!
//! Discovery is read-only and proposes candidates only — it never executes a
//! process, never returns or accepts a shell string, and never inspects an
//! untrusted script's body. Every candidate's `executable`/`arguments` must
//! come from a small, hardcoded, reviewed vocabulary (e.g. `cargo fmt
//! --check`, or `npm run <declared-script-name>` once the script's mere
//! *existence* — never its content — has been confirmed). A human must still
//! approve one candidate at a time (see
//! `crate::repository::ValidationCommandApprovalRecord`) before any future
//! Unit is allowed to run one; this Unit does not run anything.

use std::path::Path;

use chatoms_domain::ValidationCommandKind;

use crate::error::PortFailure;

/// A single structured candidate for one [`ValidationCommandKind`], proposed
/// by manifest inspection alone. `executable` is a bare program name (e.g.
/// `"cargo"`, `"npm"`), never an absolute path; `arguments` is an ordered,
/// already-tokenized argv — never a shell string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationCommandCandidate {
    pub kind: ValidationCommandKind,
    pub executable: String,
    pub arguments: Vec<String>,
}

/// Read-only manifest inspection for a task worktree. Implementations must
/// never execute a process, never return a shell string, and never surface a
/// candidate whose executable/arguments were not drawn from a fixed,
/// reviewed list.
pub trait ValidationCommandDiscovery {
    fn discover_candidates(
        &mut self,
        worktree_path: &Path,
    ) -> Result<Vec<ValidationCommandCandidate>, PortFailure>;
}
