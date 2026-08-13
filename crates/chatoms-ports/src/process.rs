use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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

/// Terminal classification of a streaming invocation. Distinct from
/// [`ProcessOutcome`] because a streaming run has two additional safe ways
/// to end early: the caller cancelled it, or its stdout exceeded the
/// caller-supplied byte bound. `Uncertain` must not be treated as success,
/// exactly as for the one-shot contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamingOutcome {
    Completed,
    Cancelled,
    StdoutBoundExceeded,
    Uncertain,
}

/// Structured result of a streaming invocation. Unlike [`ProcessCompletion`],
/// this never carries stdout/stderr bytes: stdout was already delivered
/// incrementally to the caller's [`StreamingProcessObserver`], and stderr is
/// never surfaced at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamingProcessCompletion {
    pub outcome: StreamingOutcome,
    pub exit_code: Option<i32>,
}

/// Provider-neutral, content-free lifecycle event for a streaming
/// invocation. Never carries stdout/stderr bytes, partial tokens, or
/// transcript text, so it is safe to log or surface further once a future
/// Unit wires up persistence or IPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLifecycleEvent {
    Started,
    StdoutChunkReceived { byte_len: usize },
    StdoutBoundExceeded,
    CancellationRequested,
    Exited { exit_code: Option<i32> },
}

/// Receives incremental stdout bytes and safe lifecycle events from a
/// streaming invocation. `on_stdout_chunk` bytes are transient adapter-local
/// data, exactly like [`ProcessCompletion::stdout`]: callers must not log,
/// persist, or otherwise surface them without masking sensitive content
/// first. `on_event` values carry no content and may be surfaced more
/// freely. There is no stderr callback: stderr is drained only to prevent
/// the child from blocking on a full pipe and is never exposed here.
pub trait StreamingProcessObserver {
    fn on_stdout_chunk(&mut self, chunk: &[u8]);
    fn on_event(&mut self, event: ProcessLifecycleEvent);
}

/// Cooperative cancellation signal a caller flips from another thread while
/// a [`StreamingProcessRunner::run_streaming`] call is blocked in progress.
pub trait CancellationSignal: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// A ready-made, clonable [`CancellationSignal`] backed by a shared atomic
/// flag. Cloning shares the same underlying flag; call [`Self::cancel`] from
/// any clone, on any thread, to request cancellation.
#[derive(Clone, Debug, Default)]
pub struct AtomicCancellationSignal(Arc<AtomicBool>);

impl AtomicCancellationSignal {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl CancellationSignal for AtomicCancellationSignal {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Structured, shell-free streaming process execution contract with
/// cooperative cancellation. Implementations spawn `executable` with
/// `arguments` in `working_directory`, deliver stdout to `observer`
/// incrementally and bounded by `max_stdout_bytes`, and poll `cancellation`
/// while the child runs. On cancellation, or on exceeding the stdout bound,
/// implementations terminate the child and, where the platform allows,
/// its descendants. This is a separate contract from [`ProcessRunner`] so
/// the existing one-shot capability-probe behavior never changes.
pub trait StreamingProcessRunner {
    fn run_streaming(
        &mut self,
        spec: &ProcessSpec,
        stdin: Option<&[u8]>,
        max_stdout_bytes: usize,
        cancellation: &dyn CancellationSignal,
        observer: &mut dyn StreamingProcessObserver,
    ) -> Result<StreamingProcessCompletion, PortFailure>;
}
