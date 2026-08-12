use std::{
    cell::RefCell,
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
};

use chatoms_infrastructure::provider::StdProviderCapabilityAdapter;
use chatoms_ports::{
    error::PortFailure,
    process::{ProcessCompletion, ProcessOutcome, ProcessRunner},
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
};

#[cfg(windows)]
use chatoms_platform::{
    SecureAppPaths, path::WindowsPathResolver, permissions::WindowsPermissionManager,
    preflight::TrustedPreflightWorkingDirectory,
};

type ObservedCall = (PathBuf, Vec<OsString>, PathBuf, Option<Vec<u8>>);

/// Records every call it receives and, unless told otherwise, scripts a
/// response that would satisfy every compatibility/login probe. Used to
/// prove a fast, non-process gate short-circuits *before* any process is
/// spawned: if it were reached, this runner would happily report success,
/// so a recorded call count of zero is real evidence of a skipped spawn,
/// not a coincidence of an empty script.
#[derive(Clone, Default)]
struct RecordingProcessRunner(Rc<RefCell<Vec<ObservedCall>>>);

impl RecordingProcessRunner {
    fn call_count(&self) -> usize {
        self.0.borrow().len()
    }
}

impl ProcessRunner for RecordingProcessRunner {
    fn run(
        &mut self,
        executable: &Path,
        arguments: &[OsString],
        working_directory: &Path,
        stdin: Option<&[u8]>,
    ) -> Result<ProcessCompletion, PortFailure> {
        self.0.borrow_mut().push((
            executable.to_path_buf(),
            arguments.to_vec(),
            working_directory.to_path_buf(),
            stdin.map(<[u8]>::to_vec),
        ));
        Ok(ProcessCompletion {
            outcome: ProcessOutcome::Completed,
            exit_code: Some(0),
            stdout: b"2.1.211 (Claude Code) --permission-mode plan --tools Read Glob Grep".to_vec(),
            stderr: Vec::new(),
        })
    }
}

#[cfg(windows)]
fn prepared_preflight_dir() -> (tempfile::TempDir, TrustedPreflightWorkingDirectory) {
    let temp = tempfile::tempdir().expect("independent test root");
    let resolver = WindowsPathResolver::from_base_dir_for_test(temp.path().to_path_buf())
        .expect("absolute local base");
    SecureAppPaths::prepare(&resolver, &WindowsPermissionManager)
        .expect("app-owned layout including temp_dir prepares first");
    let dir = TrustedPreflightWorkingDirectory::prepare(&resolver, &WindowsPermissionManager)
        .expect("preflight directory prepares and secures");
    (temp, dir)
}

#[test]
fn no_designated_path_is_unsupported_for_claude_and_codex() {
    let runner = RecordingProcessRunner::default();
    let mut adapter = StdProviderCapabilityAdapter::new(None, None, runner.clone());
    let capabilities = adapter
        .provider_capabilities()
        .expect("capability probe never returns a hard error");
    assert_eq!(capabilities.claude, ProviderCapabilityStatus::Unsupported);
    assert_eq!(capabilities.codex, ProviderCapabilityStatus::Unsupported);
    assert_eq!(
        runner.call_count(),
        0,
        "no designated executable must never spawn a process"
    );
}

#[test]
fn nonexistent_designated_path_is_fail_closed_not_a_hard_error() {
    let missing = std::env::temp_dir()
        .join("chatoms-provider-capability-missing-fixture")
        .join("claude.exe");
    let runner = RecordingProcessRunner::default();
    let mut adapter = StdProviderCapabilityAdapter::new(Some(missing), None, runner.clone());
    let capabilities = adapter
        .provider_capabilities()
        .expect("a missing designated path is a normal Unsupported result, not an error");
    assert_eq!(capabilities.claude, ProviderCapabilityStatus::Unsupported);
    assert_eq!(
        runner.call_count(),
        0,
        "an unverifiable executable must never spawn a process"
    );
}

#[test]
fn unsigned_regular_file_at_designated_path_is_fail_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let candidate = dir.path().join("claude.exe");
    std::fs::write(&candidate, b"not a signed binary").expect("write fixture");
    let runner = RecordingProcessRunner::default();
    let mut adapter = StdProviderCapabilityAdapter::new(Some(candidate), None, runner.clone());
    let capabilities = adapter
        .provider_capabilities()
        .expect("capability probe never returns a hard error");
    assert_eq!(capabilities.claude, ProviderCapabilityStatus::Unsupported);
    assert_eq!(
        runner.call_count(),
        0,
        "an untrusted executable must never spawn a process, even with no preflight directory at all"
    );
}

#[test]
fn codex_is_always_unsupported_regardless_of_claude_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let candidate = dir.path().join("claude.exe");
    std::fs::write(&candidate, b"not a signed binary").expect("write fixture");
    let runner = RecordingProcessRunner::default();
    let mut adapter = StdProviderCapabilityAdapter::new(Some(candidate), None, runner);
    let capabilities = adapter
        .provider_capabilities()
        .expect("capability probe never returns a hard error");
    assert_eq!(capabilities.codex, ProviderCapabilityStatus::Unsupported);
}

#[cfg(windows)]
#[test]
fn preflight_directory_replaced_with_reparse_point_fails_revalidation_and_claude_is_unsupported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let candidate = dir.path().join("claude.exe");
    std::fs::write(&candidate, b"not a signed binary").expect("write fixture");

    let (_temp, preflight_dir) = prepared_preflight_dir();
    let target = dir.path().join("junction-target");
    std::fs::create_dir(&target).expect("junction target");
    std::fs::remove_dir(preflight_dir.path()).expect("clear prepared directory for replacement");
    let junction = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(preflight_dir.path())
        .arg(&target)
        .output()
        .expect("run mklink junction fixture");
    assert!(
        junction.status.success(),
        "junction fixture is mandatory: {}",
        String::from_utf8_lossy(&junction.stderr)
    );

    let runner = RecordingProcessRunner::default();
    let mut adapter =
        StdProviderCapabilityAdapter::new(Some(candidate), Some(preflight_dir), runner.clone());
    let capabilities = adapter
        .provider_capabilities()
        .expect("capability probe never returns a hard error");
    assert_eq!(capabilities.claude, ProviderCapabilityStatus::Unsupported);
    assert_eq!(
        runner.call_count(),
        0,
        "a rebound preflight directory must never spawn a process, even though the executable is already untrusted for an unrelated reason"
    );
}

#[cfg(windows)]
#[test]
fn revalidated_preflight_directory_alone_does_not_reach_supported_without_a_trusted_executable() {
    let (_temp, preflight_dir) = prepared_preflight_dir();
    let runner = RecordingProcessRunner::default();
    let mut adapter = StdProviderCapabilityAdapter::new(None, Some(preflight_dir), runner.clone());
    let capabilities = adapter
        .provider_capabilities()
        .expect("capability probe never returns a hard error");
    assert_eq!(capabilities.claude, ProviderCapabilityStatus::Unsupported);
    assert_eq!(
        runner.call_count(),
        0,
        "a valid preflight directory with no designated executable must never spawn a process"
    );
}
