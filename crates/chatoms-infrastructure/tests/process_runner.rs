use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::mpsc::RecvTimeoutError,
    time::Duration,
};

use chatoms_infrastructure::process::StdProcessRunner;
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    process::{ProcessCompletion, ProcessOutcome, ProcessRunner},
};

#[cfg(windows)]
fn shell_invocation(script: &str) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from("cmd.exe"),
        vec![OsString::from("/C"), OsString::from(script)],
    )
}

#[cfg(not(windows))]
fn shell_invocation(script: &str) -> (PathBuf, Vec<OsString>) {
    (
        PathBuf::from("/bin/sh"),
        vec![OsString::from("-c"), OsString::from(script)],
    )
}

fn current_directory() -> PathBuf {
    std::env::current_dir().expect("current directory")
}

#[test]
fn production_runner_executes_and_captures_exit_code_and_stdout() {
    let (executable, arguments) = shell_invocation("echo hello-stdout");
    let mut runner = StdProcessRunner::new();
    let completion = runner
        .run(&executable, &arguments, &current_directory(), None)
        .expect("echo succeeds");
    assert_eq!(completion.outcome, ProcessOutcome::Completed);
    assert_eq!(completion.exit_code, Some(0));
    assert!(
        String::from_utf8_lossy(&completion.stdout).contains("hello-stdout"),
        "stdout should contain the echoed text"
    );
}

#[test]
fn stdin_is_forwarded_and_stdout_stderr_streams_remain_separate() {
    const LINE_COUNT: usize = 20_000;
    let mut input = Vec::with_capacity(LINE_COUNT * 7);
    for index in 0..LINE_COUNT {
        input.extend_from_slice(format!("payload-{index:05}\r\n").as_bytes());
    }
    let (executable, arguments) = shell_invocation("findstr /R . & echo stderr-marker-only 1>&2");
    let (sender, receiver) = std::sync::mpsc::channel();
    let runner_input = input.clone();
    let runner_executable = executable.clone();
    let runner_arguments = arguments.clone();
    let runner_cwd = current_directory();
    let worker = std::thread::spawn(move || {
        let mut runner = StdProcessRunner::new();
        let completion = runner.run(
            &runner_executable,
            &runner_arguments,
            &runner_cwd,
            Some(&runner_input),
        );
        let _ = sender.send(completion);
    });

    match receiver.recv_timeout(Duration::from_secs(30)) {
        Ok(result) => {
            worker
                .join()
                .expect("stdin forwarding worker thread must not panic");
            let completion = result.expect("stdin forwarding succeeds over a large payload");
            assert_eq!(completion.outcome, ProcessOutcome::Completed);
            assert_eq!(completion.exit_code, Some(0));
            assert!(
                completion.stdout.len() >= input.len(),
                "stdout must contain the forwarded stdin payload, got {} bytes for {} input bytes",
                completion.stdout.len(),
                input.len()
            );
            assert!(
                String::from_utf8_lossy(&completion.stdout).contains("payload-00000"),
                "stdout must contain forwarded stdin content"
            );
            assert!(
                !String::from_utf8_lossy(&completion.stdout).contains("stderr-marker-only"),
                "stdout must not contain the stderr-only marker"
            );
            assert!(
                String::from_utf8_lossy(&completion.stderr).contains("stderr-marker-only"),
                "stderr must contain its own marker"
            );
            assert!(
                !String::from_utf8_lossy(&completion.stderr).contains("payload-"),
                "stderr must not contain the forwarded stdin payload"
            );
        }
        Err(RecvTimeoutError::Disconnected) => {
            worker
                .join()
                .expect("stdin forwarding worker thread panicked before sending result");
        }
        Err(RecvTimeoutError::Timeout) => {
            panic!("stdin/stdout/stderr pipes deadlocked past the 30s bound for {LINE_COUNT} lines")
        }
    }
}

#[test]
fn non_zero_exit_code_is_reported_as_completed_not_uncertain() {
    let (executable, arguments) = shell_invocation("exit 3");
    let mut runner = StdProcessRunner::new();
    let completion = runner
        .run(&executable, &arguments, &current_directory(), None)
        .expect("a non-zero exit is still a completed invocation");
    assert_eq!(completion.outcome, ProcessOutcome::Completed);
    assert_eq!(completion.exit_code, Some(3));
}

#[test]
fn missing_executable_is_a_safe_unsupported_failure() {
    let executable = PathBuf::from("chatoms-process-runner-nonexistent-executable");
    let mut runner = StdProcessRunner::new();
    let error = runner
        .run(&executable, &[], &current_directory(), None)
        .expect_err("a missing executable must not spawn a process");
    assert_eq!(error.category(), FailureCategory::Unsupported);
}

type ObservedCall = (PathBuf, Vec<OsString>, PathBuf, Option<Vec<u8>>);

struct FakeProcessRunner {
    scripted_result: ProcessCompletion,
    observed_call: Option<ObservedCall>,
}

impl FakeProcessRunner {
    fn new(scripted_result: ProcessCompletion) -> Self {
        Self {
            scripted_result,
            observed_call: None,
        }
    }
}

impl ProcessRunner for FakeProcessRunner {
    fn run(
        &mut self,
        executable: &Path,
        arguments: &[OsString],
        working_directory: &Path,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessCompletion, chatoms_ports::error::PortFailure> {
        self.observed_call = Some((
            executable.to_path_buf(),
            arguments.to_vec(),
            working_directory.to_path_buf(),
            stdin.map(<[u8]>::to_vec),
        ));
        Ok(self.scripted_result.clone())
    }
}

fn invoke_via_port_contract(
    runner: &mut dyn ProcessRunner,
    executable: &Path,
    arguments: &[OsString],
    working_directory: &Path,
    stdin: Option<&[u8]>,
) -> Result<ProcessCompletion, chatoms_ports::error::PortFailure> {
    runner.run(executable, arguments, working_directory, stdin)
}

#[test]
fn fake_process_runner_satisfies_the_port_contract_without_a_child_process() {
    let scripted_result = ProcessCompletion {
        outcome: ProcessOutcome::Completed,
        exit_code: Some(0),
        stdout: b"scripted-stdout".to_vec(),
        stderr: b"scripted-stderr".to_vec(),
    };
    let mut fake = FakeProcessRunner::new(scripted_result.clone());
    let executable = PathBuf::from("scripted-executable");
    let arguments = vec![OsString::from("--flag"), OsString::from("value")];
    let working_directory = PathBuf::from(".");
    let stdin = b"scripted-stdin".to_vec();

    let result = invoke_via_port_contract(
        &mut fake,
        &executable,
        &arguments,
        &working_directory,
        Some(&stdin),
    )
    .expect("the fake runner returns its scripted result");

    assert_eq!(result, scripted_result);
    let observed = fake
        .observed_call
        .expect("the fake runner records the observed call");
    assert_eq!(observed.0, executable);
    assert_eq!(observed.1, arguments);
    assert_eq!(observed.2, working_directory);
    assert_eq!(observed.3, Some(stdin));
}

#[test]
fn fake_process_runner_reports_uncertain_outcome_without_running_anything() {
    let scripted_result = ProcessCompletion {
        outcome: ProcessOutcome::Uncertain,
        exit_code: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let mut fake = FakeProcessRunner::new(scripted_result.clone());
    let result = invoke_via_port_contract(
        &mut fake,
        Path::new("unused-executable"),
        &[],
        Path::new("."),
        None,
    )
    .expect("the fake runner returns its scripted uncertain result");
    assert_eq!(result, scripted_result);
    assert_eq!(result.outcome, ProcessOutcome::Uncertain);
    assert!(result.exit_code.is_none());
}
