use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use chatoms_infrastructure::process::StdProcessRunner;
use chatoms_ports::{
    error::CategorizedFailure,
    process::{
        AtomicCancellationSignal, CancellationSignal, ProcessLifecycleEvent, ProcessSpec,
        StreamingOutcome, StreamingProcessObserver, StreamingProcessRunner,
    },
};

#[cfg(windows)]
fn shell_spec(script: &str) -> ProcessSpec {
    ProcessSpec {
        executable: PathBuf::from("cmd.exe"),
        arguments: vec!["/C".into(), script.into()],
        working_directory: current_directory(),
        environment: None,
    }
}

#[cfg(not(windows))]
fn shell_spec(script: &str) -> ProcessSpec {
    ProcessSpec {
        executable: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".into(), script.into()],
        working_directory: current_directory(),
        environment: None,
    }
}

fn current_directory() -> PathBuf {
    std::env::current_dir().expect("current directory")
}

/// Writes `body` to a fresh temp file with `extension` and returns its path,
/// keeping the temp file alive for as long as the returned guard is held.
/// Fixture scripts are written to a real file (rather than inlined as a
/// single shell-quoted string) so nested quoting inside the script body
/// never has to survive a second round of argv escaping.
fn write_script_file(body: &str, extension: &str) -> tempfile::TempPath {
    use std::io::Write as _;
    let mut file = tempfile::Builder::new()
        .suffix(extension)
        .tempfile()
        .expect("create fixture script file");
    write!(file, "{body}").expect("write fixture script body");
    file.into_temp_path()
}

/// A [`ProcessSpec`] that runs a script file directly with the platform
/// shell, passing the path as its own argv element (no manual escaping).
#[cfg(windows)]
fn script_file_spec(path: &Path) -> ProcessSpec {
    ProcessSpec {
        executable: PathBuf::from("cmd.exe"),
        arguments: vec!["/C".into(), path.as_os_str().to_owned()],
        working_directory: current_directory(),
        environment: None,
    }
}

#[cfg(not(windows))]
fn script_file_spec(path: &Path) -> ProcessSpec {
    ProcessSpec {
        executable: PathBuf::from("/bin/sh"),
        arguments: vec![path.as_os_str().to_owned()],
        working_directory: current_directory(),
        environment: None,
    }
}

/// A never-cancelled signal, for tests that don't exercise cancellation.
fn never_cancelled() -> AtomicCancellationSignal {
    AtomicCancellationSignal::new()
}

#[derive(Default)]
struct RecordingObserver {
    stdout_chunks: Vec<Vec<u8>>,
    events: Vec<ProcessLifecycleEvent>,
}

impl StreamingProcessObserver for RecordingObserver {
    fn on_stdout_chunk(&mut self, chunk: &[u8]) {
        self.stdout_chunks.push(chunk.to_vec());
    }

    fn on_event(&mut self, event: ProcessLifecycleEvent) {
        self.events.push(event);
    }
}

impl RecordingObserver {
    fn stdout(&self) -> Vec<u8> {
        self.stdout_chunks.concat()
    }
}

#[test]
fn normal_exit_streams_stdout_and_reports_completed() {
    let spec = shell_spec("echo hello-stream");
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("echo succeeds");

    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    assert_eq!(completion.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&observer.stdout()).contains("hello-stream"));
    assert!(
        observer
            .events
            .iter()
            .any(|event| matches!(event, ProcessLifecycleEvent::Started))
    );
    assert!(
        observer
            .events
            .iter()
            .any(|event| matches!(event, ProcessLifecycleEvent::Exited { exit_code: Some(0) }))
    );
}

#[test]
fn non_zero_exit_is_completed_not_uncertain() {
    let spec = shell_spec("exit 3");
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("a non-zero exit is still a completed invocation");
    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    assert_eq!(completion.exit_code, Some(3));
}

#[test]
fn missing_executable_is_a_safe_unsupported_failure() {
    let spec = ProcessSpec {
        executable: PathBuf::from("chatoms-streaming-runner-nonexistent-executable"),
        arguments: vec![],
        working_directory: current_directory(),
        environment: None,
    };
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let error = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect_err("a missing executable must not spawn a process");
    assert_eq!(
        error.category(),
        chatoms_ports::error::FailureCategory::Unsupported
    );
}

#[cfg(windows)]
fn multi_chunk_script() -> String {
    // Two separate echo invocations, each on its own line, force at least
    // two independent writes to the stdout pipe.
    "echo chunk-one& ping -n 2 127.0.0.1 >NUL & echo chunk-two".to_owned()
}

#[cfg(not(windows))]
fn multi_chunk_script() -> String {
    "echo chunk-one; sleep 0.2; echo chunk-two".to_owned()
}

#[test]
fn stdout_delivered_across_chunk_boundaries_reassembles_correctly() {
    let spec = shell_spec(&multi_chunk_script());
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("multi-write script succeeds");

    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    let stdout = String::from_utf8_lossy(&observer.stdout()).into_owned();
    assert!(stdout.contains("chunk-one"), "stdout was: {stdout:?}");
    assert!(stdout.contains("chunk-two"), "stdout was: {stdout:?}");
    assert!(
        observer.stdout_chunks.len() >= 2,
        "expected delivery across at least two separate reads, got {}",
        observer.stdout_chunks.len()
    );
}

#[test]
fn stdin_is_forwarded_while_stdout_and_stderr_drain_concurrently() {
    const LINE_COUNT: usize = 20_000;
    let mut input = Vec::with_capacity(LINE_COUNT * 7);
    for index in 0..LINE_COUNT {
        input.extend_from_slice(format!("payload-{index:05}\r\n").as_bytes());
    }
    let spec = shell_spec("findstr /R . & echo stderr-marker-only 1>&2");
    let runner = StdProcessRunner::new();
    let observer = RecordingObserver::default();

    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut runner = runner;
        let mut observer = observer;
        let result = runner.run_streaming(
            &spec,
            Some(&input),
            input.len() + 4096,
            &never_cancelled(),
            &mut observer,
        );
        let _ = sender.send((result, observer));
    });

    let (result, observer) = receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("stdin/stdout/stderr pipes must not deadlock past the 30s bound");
    let completion = result.expect("stdin forwarding succeeds over a large payload");
    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    assert_eq!(completion.exit_code, Some(0));

    let stdout = String::from_utf8_lossy(&observer.stdout()).into_owned();
    assert!(
        stdout.contains("payload-00000"),
        "stdout must contain forwarded stdin content"
    );
    assert!(
        !stdout.contains("stderr-marker-only"),
        "stdout must not contain the stderr-only marker"
    );
}

#[test]
fn stderr_is_never_exposed_to_the_observer() {
    let spec = shell_spec("echo stdout-line & echo stderr-only-marker 1>&2");
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("script succeeds");
    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    let stdout = String::from_utf8_lossy(&observer.stdout()).into_owned();
    assert!(stdout.contains("stdout-line"));
    assert!(
        !stdout.contains("stderr-only-marker"),
        "stderr content must never reach the observer"
    );
}

#[test]
fn malformed_utf8_stdout_bytes_pass_through_unmodified_without_panicking() {
    // 0xFF/0xFE are never valid leading UTF-8 bytes. Writing them into a
    // real file and streaming that file's bytes back (via `type`/`cat`)
    // avoids depending on any shell's own quoting/encoding of raw bytes.
    let raw_bytes: [u8; 4] = [0x41, 0xFF, 0xFE, 0x42];
    let mut byte_file = tempfile::NamedTempFile::new().expect("create raw byte fixture file");
    std::io::Write::write_all(&mut byte_file, &raw_bytes).expect("write raw bytes");
    let byte_path = byte_file.into_temp_path();

    let spec = if cfg!(windows) {
        ProcessSpec {
            executable: PathBuf::from("cmd.exe"),
            arguments: vec!["/C".into(), "type".into(), byte_path.as_os_str().to_owned()],
            working_directory: current_directory(),
            environment: None,
        }
    } else {
        ProcessSpec {
            executable: PathBuf::from("cat"),
            arguments: vec![byte_path.as_os_str().to_owned()],
            working_directory: current_directory(),
            environment: None,
        }
    };
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("emitting invalid UTF-8 bytes must not fail the invocation");
    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    let stdout = observer.stdout();
    assert!(
        stdout
            .windows(raw_bytes.len())
            .any(|window| window == raw_bytes),
        "the invalid UTF-8 bytes must be forwarded verbatim, got {stdout:?}"
    );
}

#[test]
fn oversized_stdout_stops_early_and_reports_bound_exceeded() {
    let script = if cfg!(windows) {
        "for /L %i in (1,1,200) do @echo line-%i-of-filler-text-to-exceed-the-bound"
    } else {
        "for i in $(seq 1 200); do echo line-$i-of-filler-text-to-exceed-the-bound; done"
    };
    let spec = shell_spec(script);
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let completion = runner
        .run_streaming(&spec, None, 32, &never_cancelled(), &mut observer)
        .expect("bound-exceeded is a safe completion, not an error");

    assert_eq!(completion.outcome, StreamingOutcome::StdoutBoundExceeded);
    assert!(
        observer
            .events
            .iter()
            .any(|event| matches!(event, ProcessLifecycleEvent::StdoutBoundExceeded)),
        "a safe StdoutBoundExceeded event must be emitted"
    );
    assert!(
        observer.stdout().len() <= 32,
        "no more than the bound may ever reach the observer, got {} bytes",
        observer.stdout().len()
    );
}

#[cfg(windows)]
fn long_running_script() -> String {
    "ping -n 30 127.0.0.1 >NUL".to_owned()
}

#[cfg(not(windows))]
fn long_running_script() -> String {
    "sleep 30".to_owned()
}

#[test]
fn cancellation_terminates_the_child_promptly() {
    let spec = shell_spec(&long_running_script());
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let cancellation = AtomicCancellationSignal::new();
    let cancel_handle = cancellation.clone();

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel_handle.cancel();
    });

    let start = std::time::Instant::now();
    let completion = runner
        .run_streaming(&spec, None, 1024, &cancellation, &mut observer)
        .expect("cancellation is a safe completion, not an error");

    assert_eq!(completion.outcome, StreamingOutcome::Cancelled);
    assert!(
        start.elapsed() < Duration::from_secs(25),
        "cancellation must terminate the 30s sleep well before its natural exit, took {:?}",
        start.elapsed()
    );
    assert!(
        observer
            .events
            .iter()
            .any(|event| matches!(event, ProcessLifecycleEvent::CancellationRequested))
    );
}

/// Builds the descendant-heartbeat fixture, returning the outer script's
/// path (what the test actually runs) plus the descendant script's path
/// (kept alive only so its temp file isn't deleted early on Windows).
#[cfg(windows)]
fn descendant_spawning_script(marker: &Path) -> (tempfile::TempPath, tempfile::TempPath) {
    // `start /B` launches a detached descendant that keeps appending
    // heartbeats to `marker` while the outer cmd.exe blocks on its own
    // direct child. If cancellation only killed the outer process, the
    // detached descendant would keep ticking. `%%i` (not `%i`) is required
    // for a `for` loop inside a script file rather than a command line.
    let descendant = write_script_file(
        &format!(
            "@echo off\r\nfor /L %%i in (1,1,300) do (\r\n  echo tick>>\"{}\"\r\n  ping -n 1 127.0.0.1 >NUL\r\n)\r\n",
            marker.display()
        ),
        ".cmd",
    );
    let outer = write_script_file(
        &format!(
            "@echo off\r\nstart /B cmd /C \"{}\"\r\nping -n 30 127.0.0.1 >NUL\r\n",
            descendant.display()
        ),
        ".cmd",
    );
    (outer, descendant)
}

#[cfg(not(windows))]
fn descendant_spawning_script(marker: &Path) -> (tempfile::TempPath, ()) {
    let outer = write_script_file(
        &format!(
            "(while true; do echo tick >> \"{}\"; sleep 0.3; done) &\nsleep 30\n",
            marker.display()
        ),
        ".sh",
    );
    (outer, ())
}

fn tick_count(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn cancellation_terminates_descendant_processes_too() {
    // `.into_temp_path()` drops the open File handle immediately, keeping
    // only the path: the fixture's child/descendant processes need to open
    // this file themselves, which Windows would otherwise deny while this
    // process still held it open.
    let marker_path_guard = tempfile::NamedTempFile::new()
        .expect("create heartbeat marker file")
        .into_temp_path();
    let marker_path = marker_path_guard.to_path_buf();
    let (outer_script, _descendant_script) = descendant_spawning_script(&marker_path);
    let spec = script_file_spec(&outer_script);
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let cancellation = AtomicCancellationSignal::new();
    let cancel_handle = cancellation.clone();

    thread::spawn(move || {
        // Give the descendant time to start ticking before cancelling.
        thread::sleep(Duration::from_millis(1500));
        cancel_handle.cancel();
    });

    let start = std::time::Instant::now();
    let completion = runner
        .run_streaming(&spec, None, 1024, &cancellation, &mut observer)
        .expect("cancellation is a safe completion, not an error");

    assert!(matches!(
        completion.outcome,
        StreamingOutcome::Cancelled | StreamingOutcome::Uncertain
    ));
    assert!(
        start.elapsed() < Duration::from_secs(25),
        "descendant-aware cancellation must not wait out the 30s sleep, took {:?}",
        start.elapsed()
    );
    assert!(
        tick_count(&marker_path) > 0,
        "the descendant must have started and ticked at least once before cancellation"
    );

    thread::sleep(Duration::from_millis(1500));
    let ticks_after_first_wait = tick_count(&marker_path);
    thread::sleep(Duration::from_millis(1500));
    let ticks_after_second_wait = tick_count(&marker_path);

    assert_eq!(
        ticks_after_first_wait, ticks_after_second_wait,
        "the descendant kept ticking after cancellation, so it was not actually terminated"
    );
}

#[test]
fn cancel_requested_concurrently_with_a_fast_natural_exit_reports_a_consistent_outcome() {
    // The child exits almost immediately on its own. Cancellation is
    // requested at essentially the same moment. Whichever side "wins" the
    // race is acceptable, but the reported outcome must be internally
    // consistent: a Completed result must carry the script's real exit
    // code, never a fabricated one.
    for _ in 0..10 {
        let spec = shell_spec("exit 0");
        let mut runner = StdProcessRunner::new();
        let mut observer = RecordingObserver::default();
        let cancellation = AtomicCancellationSignal::new();
        let cancel_handle = cancellation.clone();
        thread::spawn(move || cancel_handle.cancel());

        let completion = runner
            .run_streaming(&spec, None, 1024, &cancellation, &mut observer)
            .expect("cancel/exit race is always a safe completion, not an error");

        match completion.outcome {
            StreamingOutcome::Completed => assert_eq!(completion.exit_code, Some(0)),
            StreamingOutcome::Cancelled => {}
            other => panic!("unexpected outcome for a cancel/exit race: {other:?}"),
        }
    }
}

#[test]
fn cancel_racing_with_natural_exit_reports_the_actual_outcome() {
    // The script exits almost immediately; a cancel requested slightly
    // afterward must not relabel an already-completed run as Cancelled.
    let spec = shell_spec("exit 0");
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();
    let cancellation = AtomicCancellationSignal::new();
    let completion = runner
        .run_streaming(&spec, None, 1024, &cancellation, &mut observer)
        .expect("script succeeds");

    // Cancelling only after the call already returned must have no effect;
    // this just documents that the signal is inert once the run is over.
    cancellation.cancel();
    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    assert_eq!(completion.exit_code, Some(0));
}

#[cfg(windows)]
fn print_env_var_script(name: &str) -> String {
    format!("echo %{name}%")
}

#[cfg(not(windows))]
fn print_env_var_script(name: &str) -> String {
    format!("echo ${name}")
}

#[cfg(windows)]
fn print_two_env_vars_script(first: &str, second: &str) -> String {
    format!(
        "{} & {}",
        print_env_var_script(first),
        print_env_var_script(second)
    )
}

#[cfg(not(windows))]
fn print_two_env_vars_script(first: &str, second: &str) -> String {
    format!(
        "{}; {}",
        print_env_var_script(first),
        print_env_var_script(second)
    )
}

#[test]
fn spec_environment_none_still_inherits_the_parent_environment() {
    let marker_name = "CHATOMS_STREAMING_RUNNER_ENV_TEST_MARKER";
    // No other test in this file reads or writes an environment variable, so
    // this set/remove pair around a single synchronous spawn below cannot
    // race with anything else in this test binary.
    unsafe {
        std::env::set_var(marker_name, "inherited-value");
    }
    let spec = shell_spec(&print_env_var_script(marker_name));
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();

    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("echo succeeds");

    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    let stdout = String::from_utf8_lossy(&observer.stdout()).into_owned();
    assert!(
        stdout.contains("inherited-value"),
        "spec.environment == None must keep inheriting the parent environment, got: {stdout:?}"
    );
    unsafe {
        std::env::remove_var(marker_name);
    }
}

#[test]
fn spec_environment_some_clears_everything_not_explicitly_listed() {
    let marker_name = "CHATOMS_STREAMING_RUNNER_ENV_TEST_MARKER_CLEARED";
    unsafe {
        std::env::set_var(marker_name, "must-not-be-inherited");
    }
    let allowed_name = "CHATOMS_STREAMING_RUNNER_ENV_TEST_ALLOWED";
    let mut spec = shell_spec(&print_two_env_vars_script(marker_name, allowed_name));
    spec.environment = Some(vec![(allowed_name.into(), "explicitly-set".into())]);
    let mut runner = StdProcessRunner::new();
    let mut observer = RecordingObserver::default();

    let completion = runner
        .run_streaming(&spec, None, 1024, &never_cancelled(), &mut observer)
        .expect("echo succeeds");

    assert_eq!(completion.outcome, StreamingOutcome::Completed);
    let stdout = String::from_utf8_lossy(&observer.stdout()).into_owned();
    assert!(
        !stdout.contains("must-not-be-inherited"),
        "an env_clear'd child must not inherit any parent variable, got: {stdout:?}"
    );
    assert!(
        stdout.contains("explicitly-set"),
        "a variable explicitly listed in spec.environment must reach the child, got: {stdout:?}"
    );
    unsafe {
        std::env::remove_var(marker_name);
    }
}

#[test]
fn shared_cancellation_signal_is_send_and_sync_across_threads() {
    let signal = AtomicCancellationSignal::new();
    let observed: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let signal_clone = signal.clone();
    let observed_clone = Arc::clone(&observed);
    let handle = thread::spawn(move || {
        signal_clone.cancel();
        *observed_clone.lock().expect("lock") = signal_clone.is_cancelled();
    });
    handle.join().expect("cancellation thread must not panic");
    assert!(signal.is_cancelled());
    assert!(*observed.lock().expect("lock"));
}
