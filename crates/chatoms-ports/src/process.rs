use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::error::PortFailure;

/// Structured invocation request. Defining this type spawns nothing; a
/// future `ProcessRunner` port owns spawning, streaming, and cancellation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSpec {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: PathBuf,
}

/// Whether a completed invocation's effect could be confirmed, mirroring
/// the `Created`/`NoEffect`/`Uncertain` shape already used by
/// [`crate::git::WorktreeCreationOutcome`]. `Uncertain` must not be treated
/// as success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessOutcome {
    Completed,
    Uncertain,
}

/// Structured result of a synchronous one-shot process invocation. `stdout`
/// and `stderr` are the full captured byte streams; callers must treat them
/// as transient adapter-local data and must not log, persist, or otherwise
/// surface them without masking sensitive content first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessCompletion {
    pub outcome: ProcessOutcome,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Minimal synchronous one-shot process execution contract. Implementations
/// spawn `executable` with `arguments` in `working_directory`, optionally
/// writing `stdin` to the child before it exits, and return once the child
/// has exited. This is a one-shot boundary only: cancellation, timeouts, and
/// streaming are separate future ports so this contract never requires
/// them. Implementations must not accept or parse shell strings.
pub trait ProcessRunner {
    fn run(
        &mut self,
        executable: &Path,
        arguments: &[OsString],
        working_directory: &Path,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessCompletion, PortFailure>;
}
