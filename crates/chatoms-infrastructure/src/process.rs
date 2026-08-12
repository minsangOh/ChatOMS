use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
};

use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    process::{ProcessCompletion, ProcessOutcome, ProcessRunner},
};

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
