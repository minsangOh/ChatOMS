use std::{
    ffi::OsString,
    io::{Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    process::{
        CancellationSignal, ProcessCompletion, ProcessLifecycleEvent, ProcessOutcome,
        ProcessRunner, ProcessSpec, StreamingOutcome, StreamingProcessCompletion,
        StreamingProcessObserver, StreamingProcessRunner,
    },
};

/// Size of each incremental read from the child's stdout pipe.
const STDOUT_READ_CHUNK_BYTES: usize = 8192;
/// Interval at which [`StdProcessRunner::run_streaming`] polls for exit,
/// new stdout data, and cancellation.
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(15);
/// Once termination has been requested, the longest this runner waits for
/// the OS to confirm the child actually exited before fail-closing to
/// [`StreamingOutcome::Uncertain`].
const CANCEL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

/// Synchronous one-shot [`ProcessRunner`] built only on `std::process`.
/// Spawns `executable` with `arguments` in `working_directory`, optionally
/// writing `stdin` to the child on a dedicated thread while stdout/stderr
/// are drained concurrently by `wait_with_output`, so neither pipe direction
/// can deadlock the other.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdProcessRunner;

impl StdProcessRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ProcessRunner for StdProcessRunner {
    fn run(
        &mut self,
        executable: &Path,
        arguments: &[OsString],
        working_directory: &Path,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessCompletion, PortFailure> {
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .current_dir(working_directory)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(map_spawn_error)?;
        let Some(input) = stdin else {
            let output = child.wait_with_output().map_err(map_io_error)?;
            return Ok(completion_from_output(output));
        };
        let mut stdin_handle = child
            .stdin
            .take()
            .ok_or_else(|| PortFailure::new(FailureCategory::Internal))?;
        thread::scope(|scope| {
            let writer = scope.spawn(move || stdin_handle.write_all(input));
            let output = child.wait_with_output().map_err(map_io_error);
            match writer.join() {
                Ok(Ok(())) => output.map(completion_from_output),
                Ok(Err(write_error)) => Err(map_io_error(write_error)),
                Err(_) => Err(PortFailure::new(FailureCategory::Internal)),
            }
        })
    }
}

impl StreamingProcessRunner for StdProcessRunner {
    fn run_streaming(
        &mut self,
        spec: &ProcessSpec,
        stdin: Option<&[u8]>,
        max_stdout_bytes: usize,
        cancellation: &dyn CancellationSignal,
        observer: &mut dyn StreamingProcessObserver,
    ) -> Result<StreamingProcessCompletion, PortFailure> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.arguments)
            .current_dir(&spec.working_directory)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        isolate_into_new_process_group(&mut command);

        let mut child = command.spawn().map_err(map_spawn_error)?;
        let pid = child.id();
        observer.on_event(ProcessLifecycleEvent::Started);

        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| PortFailure::new(FailureCategory::Internal))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| PortFailure::new(FailureCategory::Internal))?;
        let stdin_pipe = child.stdin.take();

        let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>();
        let stdout_thread = thread::spawn(move || drain_stdout_chunks(stdout_pipe, &stdout_tx));
        let stderr_thread = thread::spawn(move || drain_and_discard(stderr_pipe));
        let stdin_thread = stdin.and_then(|input| {
            stdin_pipe.map(|mut handle| {
                let input = input.to_vec();
                thread::spawn(move || handle.write_all(&input))
            })
        });

        let mut stdout_bytes_total = 0_usize;
        let mut bound_exceeded = false;
        let mut cancel_issued = false;
        let mut cancel_issued_at: Option<Instant> = None;

        let wait_result = loop {
            while let Ok(chunk) = stdout_rx.try_recv() {
                forward_stdout_chunk(
                    observer,
                    &mut stdout_bytes_total,
                    &mut bound_exceeded,
                    max_stdout_bytes,
                    &chunk,
                );
            }

            match child.try_wait() {
                Ok(Some(status)) => break Ok(Some(status)),
                Ok(None) => {}
                Err(error) => break Err(error),
            }

            if !cancel_issued && (bound_exceeded || cancellation.is_cancelled()) {
                cancel_issued = true;
                cancel_issued_at = Some(Instant::now());
                if !bound_exceeded {
                    observer.on_event(ProcessLifecycleEvent::CancellationRequested);
                }
                terminate_process_tree(pid, &mut child);
            } else if let Some(issued_at) = cancel_issued_at
                && issued_at.elapsed() > CANCEL_CONFIRM_TIMEOUT
            {
                // The OS never confirmed the child's death within the
                // fail-closed bound; report Uncertain rather than guessing.
                break Ok(None);
            }

            thread::sleep(CANCELLATION_POLL_INTERVAL);
        };

        // A final non-blocking drain catches any trailing chunk that was
        // queued before the child's exit was observed above.
        while let Ok(chunk) = stdout_rx.try_recv() {
            forward_stdout_chunk(
                observer,
                &mut stdout_bytes_total,
                &mut bound_exceeded,
                max_stdout_bytes,
                &chunk,
            );
        }

        stdout_thread
            .join()
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        stderr_thread
            .join()
            .map_err(|_| PortFailure::new(FailureCategory::Internal))?;
        if let Some(stdin_thread) = stdin_thread {
            match stdin_thread.join() {
                Ok(Ok(())) => {}
                // A broken pipe means the child exited before consuming all
                // of stdin, which is an expected outcome, not a failure.
                Ok(Err(write_error)) if write_error.kind() == std::io::ErrorKind::BrokenPipe => {}
                Ok(Err(write_error)) => return Err(map_io_error(write_error)),
                Err(_) => return Err(PortFailure::new(FailureCategory::Internal)),
            }
        }

        let exit_status = match wait_result {
            Ok(Some(status)) => Some(status),
            Ok(None) => None,
            Err(error) => return Err(map_io_error(error)),
        };
        let exit_code = exit_status.and_then(|status| status.code());
        observer.on_event(ProcessLifecycleEvent::Exited { exit_code });

        // `bound_exceeded` takes priority even if the child also happened to
        // exit naturally in the same tick: stdout was already cut short, so
        // this can never be reported as a plain `Completed`.
        let outcome = if bound_exceeded {
            StreamingOutcome::StdoutBoundExceeded
        } else if cancel_issued {
            if exit_status.is_some() {
                StreamingOutcome::Cancelled
            } else {
                StreamingOutcome::Uncertain
            }
        } else if exit_code.is_some() {
            StreamingOutcome::Completed
        } else {
            StreamingOutcome::Uncertain
        };

        Ok(StreamingProcessCompletion { outcome, exit_code })
    }
}

fn forward_stdout_chunk(
    observer: &mut dyn StreamingProcessObserver,
    stdout_bytes_total: &mut usize,
    bound_exceeded: &mut bool,
    max_stdout_bytes: usize,
    chunk: &[u8],
) {
    if *bound_exceeded {
        return;
    }
    if *stdout_bytes_total + chunk.len() > max_stdout_bytes {
        *bound_exceeded = true;
        observer.on_event(ProcessLifecycleEvent::StdoutBoundExceeded);
        return;
    }
    *stdout_bytes_total += chunk.len();
    observer.on_stdout_chunk(chunk);
    observer.on_event(ProcessLifecycleEvent::StdoutChunkReceived {
        byte_len: chunk.len(),
    });
}

fn drain_stdout_chunks(mut pipe: impl Read, sender: &mpsc::Sender<Vec<u8>>) {
    let mut buffer = [0_u8; STDOUT_READ_CHUNK_BYTES];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(read_bytes) => {
                if sender.send(buffer[..read_bytes].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn drain_and_discard(mut pipe: impl Read) {
    let mut buffer = [0_u8; STDOUT_READ_CHUNK_BYTES];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// Isolates the child (and, on platforms that support it, its future
/// descendants) from this process's own process group, so a targeted
/// termination signal can reach the whole tree without also affecting this
/// process.
fn isolate_into_new_process_group(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = command;
    }
}

/// Best-effort termination of the child and, where the platform allows, its
/// descendants. Always also calls the direct `kill` on `child` as a
/// fallback, since the tree-kill utility below may itself be unavailable.
fn terminate_process_tree(pid: u32, child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            let taskkill = Path::new(&system_root)
                .join("System32")
                .join("taskkill.exe");
            if taskkill.is_file() {
                let _ = Command::new(taskkill)
                    .args(["/T", "/F", "/PID", &pid.to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
    #[cfg(unix)]
    {
        for candidate in ["/bin/kill", "/usr/bin/kill"] {
            let path = Path::new(candidate);
            if path.is_file() {
                let _ = Command::new(path)
                    .args(["-KILL", &format!("-{pid}")])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                break;
            }
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
    let _ = child.kill();
}

fn completion_from_output(output: Output) -> ProcessCompletion {
    let exit_code = output.status.code();
    let outcome = if exit_code.is_some() {
        ProcessOutcome::Completed
    } else {
        ProcessOutcome::Uncertain
    };
    ProcessCompletion {
        outcome,
        exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

fn map_spawn_error(error: std::io::Error) -> PortFailure {
    if error.kind() == std::io::ErrorKind::NotFound {
        PortFailure::new(FailureCategory::Unsupported)
    } else {
        map_io_error(error)
    }
}

fn map_io_error(error: std::io::Error) -> PortFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => PortFailure::new(FailureCategory::NotFound),
        std::io::ErrorKind::PermissionDenied => PortFailure::new(FailureCategory::PermissionDenied),
        _ => PortFailure::new(FailureCategory::Conflict),
    }
}
