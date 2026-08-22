//! Port boundary for actually executing one already-approved validation
//! command (see `crate::validation` for the separate read-only
//! discovery/approval port this complements, and
//! `crate::repository::ValidationCommandApprovalRecord` for what "approved"
//! means).
//!
//! Execution-only: an implementation of [`ValidationCommandExecutor`] must
//! re-verify every identity binding fresh immediately before spawning
//! anything, must never accept or construct a shell string, and must reduce
//! whatever happened to the small, fail-closed vocabulary below. This Unit's
//! implementations never persist a result and never touch `Task` state —
//! that is a later Unit's responsibility, mirroring how
//! `crate::implementation::ClaudeImplementationExecutor` arrived before
//! `TaskService::record_implementation_result` existed.

use crate::{
    error::PortFailure, filesystem::DirectoryIdentity, process::CancellationSignal,
    repository::ValidationCommandApprovalRecord,
};
use chatoms_domain::{ProjectId, ValidationExecutionScope};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationExecutionTarget {
    TaskWorktree {
        directory_identity: DirectoryIdentity,
    },
    ProjectRoot {
        project_id: ProjectId,
        project_identity_revision: u64,
        directory_identity: DirectoryIdentity,
    },
}

impl ValidationExecutionTarget {
    #[must_use]
    pub const fn scope(&self) -> ValidationExecutionScope {
        match self {
            Self::TaskWorktree { .. } => ValidationExecutionScope::TaskWorktree,
            Self::ProjectRoot { .. } => ValidationExecutionScope::ProjectRoot,
        }
    }

    #[must_use]
    pub const fn directory_identity(&self) -> &DirectoryIdentity {
        match self {
            Self::TaskWorktree { directory_identity }
            | Self::ProjectRoot {
                directory_identity, ..
            } => directory_identity,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ValidationExecutionRequest<'a> {
    pub target: &'a ValidationExecutionTarget,
    pub approval: &'a ValidationCommandApprovalRecord,
}

/// Reason a validation command attempt was rejected before any subprocess
/// was spawned. Every variant means "no process started."
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationBindingRejection {
    /// The approved executable, its tool directory, or an approved
    /// environment directory binding (e.g. `CARGO_HOME`) could not be
    /// re-inspected, or its current identity no longer matches what was
    /// approved. Implementations must treat an inspection error (including
    /// a reparse point/symlink, which [`crate::filesystem::FilesystemIdentityPort`]
    /// already rejects) identically to a mismatch.
    IdentityMismatch,
    /// The approved executable's current canonical path resolves inside the
    /// explicit execution target passed to this attempt.
    ExecutableInsideExecutionTarget,
    BindingInsideExecutionTarget,
    /// `approval`'s `(executable, arguments)` does not exactly match this
    /// implementation's own fixed vocabulary for `approval.kind` — a
    /// defense-in-depth re-check that never trusts a stored row blindly.
    UnapprovedCommandKind,
    UnsupportedExecutionScope,
}

/// Terminal, fail-closed classification of one validation command attempt
/// that actually spawned. There is no plain `Failed`: a nonzero exit is its
/// own explicit variant (`ExitFailure`) rather than folded into `Success`,
/// so a future recorder never has to remember to check `exit_code == 0`
/// itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationExecutionOutcome {
    Success,
    ExitFailure { exit_code: i32 },
    TimedOut,
    StdoutBoundExceeded,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationExecutionStartOutcome {
    Completed(ValidationExecutionOutcome),
    BindingRejected(ValidationBindingRejection),
}

/// Provider/tool-neutral execution contract for one already-approved
/// validation command. The request couples one approval to one explicit,
/// scope-matched target identity. Implementations own every identity
/// re-check, controlled-environment construction, and timeout decision;
/// `crate::validation`'s discovery/approval port never executes anything.
pub trait ValidationCommandExecutor {
    fn start_validation_command(
        &mut self,
        request: ValidationExecutionRequest<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ValidationExecutionStartOutcome, PortFailure>;
}
