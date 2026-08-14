//! Cargo-only validation command execution adapter.
//!
//! Wires one already-approved [`ValidationCommandApprovalRecord`] to the
//! Unit 3 [`StreamingProcessRunner`] port, restricted for this Unit to the
//! four fixed, non-mutating Cargo subcommands
//! `crate::validation_discovery::ManifestValidationCommandDiscovery` ever
//! proposes (`fmt --all --check`, `clippy --workspace --all-targets
//! --all-features`, `test --workspace`, `build --workspace`). Package-manager
//! `run <script>` execution, `AutoFixing`, and Claude/Codex CLI execution are
//! all out of scope for this Unit.
//!
//! This Unit adds only the adapter. It never persists a result and never
//! drives a `Testing -> Reviewing`/`RecoveryRequired`/`Paused` state
//! transition — those are later Units, mirroring how Claude Planning's and
//! Claude Implementation's adapters arrived before their respective
//! orchestration/state-transition Units did.
//!
//! Three safety properties are structural, not just disciplined:
//!
//! * [`CargoValidationAdapter::start_validation_command`] re-verifies the
//!   approved executable's, its tool directory's, and any approved
//!   `CARGO_HOME`/`RUSTUP_HOME` binding's Windows stable NTFS object
//!   identity via [`FilesystemIdentityPort`] immediately before every spawn
//!   attempt — the approval-time snapshot is never trusted on its own.
//!   Every one of these bindings, including `CARGO_HOME`/`RUSTUP_HOME`, is
//!   read from the durable, user-approved `approval` argument itself (see
//!   `0012_validation_command_environment_binding.sql`), never from a value
//!   supplied separately by this adapter's caller — otherwise the
//!   "re-verification" would only compare a value against itself.
//! * `approval.executable`/`approval.arguments` are re-checked against this
//!   module's own fixed Cargo vocabulary (see [`expected_cargo_arguments`])
//!   before being trusted, independent of whatever validation the approval
//!   flow already performed when the row was written.
//! * The spawned process's environment is never inherited: `ProcessSpec`
//!   carries `environment: Some(vars)`, which
//!   `StdProcessRunner::run_streaming` honors by calling `env_clear()` and
//!   setting only `vars` — `PATH` (the approved tool directory only),
//!   `SystemRoot`, the app-owned `TEMP`/`TMP`, and, only when the approval
//!   carries one and it is freshly re-verified, `CARGO_HOME`/`RUSTUP_HOME`.
//!
//! Residual risk this Unit does not and cannot technically close: `cargo
//! test`/`cargo build` still compile and run the worktree's own Rust code
//! (`build.rs`, proc macros, `#[test]` bodies), which can reach the network,
//! spawn arbitrary child processes, or mutate Git, exactly as it would for a
//! human contributor running the same command by hand. See
//! `docs/DECISIONS.md`'s "Cargo-only validation execution scope" entry.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chatoms_domain::ValidationCommandKind;
use chatoms_ports::{
    error::PortFailure,
    filesystem::FilesystemIdentityPort,
    process::{
        CancellationSignal, ProcessLifecycleEvent, ProcessSpec, StreamingOutcome,
        StreamingProcessCompletion, StreamingProcessObserver, StreamingProcessRunner,
    },
    repository::ValidationCommandApprovalRecord,
    validation_execution::{
        ValidationBindingRejection, ValidationCommandExecutor, ValidationExecutionOutcome,
        ValidationExecutionStartOutcome,
    },
};

/// The fixed argv name-checked against every approval before it is trusted.
/// No PATH search ever happens: `approval.approved_executable_path` (an
/// absolute path, re-verified by identity below) is what is actually
/// spawned — this constant only names the logical executable an approval
/// must declare.
const CARGO_EXECUTABLE_NAME: &str = "cargo";

/// Bound on how much stdout this adapter will ever let `run_streaming`
/// deliver before treating the run as exceeding its output budget. Cargo
/// output is not parsed here (this Unit performs no result-content
/// classification beyond the process exit itself), so this bound exists only
/// to force termination of a pathologically chatty command.
const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;

/// Wall-clock cap for the shorter validation kinds. Cargo's own
/// `fmt --check`/`clippy` do not compile the full dependency graph the way
/// `test`/`build` do, so a much shorter deadline is safe.
const SHORT_KIND_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Wall-clock cap for `Test`/`Build`, which may need a full (possibly cold)
/// compile of the workspace.
const LONG_KIND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Adapter that runs one approved Cargo validation command through a
/// [`StreamingProcessRunner`], re-verifying every identity binding fresh on
/// every attempt.
pub struct CargoValidationAdapter<S, F> {
    streaming: S,
    filesystem: F,
    app_temp_dir: PathBuf,
}

impl<S, F> CargoValidationAdapter<S, F>
where
    S: StreamingProcessRunner,
    F: FilesystemIdentityPort,
{
    #[must_use]
    pub const fn new(streaming: S, filesystem: F, app_temp_dir: PathBuf) -> Self {
        Self {
            streaming,
            filesystem,
            app_temp_dir,
        }
    }

    /// Re-verifies the approval's fixed Cargo vocabulary and every identity
    /// binding, then — only if every check passes — spawns exactly the
    /// approved executable with the approved argv, `worktree_path` as CWD,
    /// and a fully `env_clear`'d, minimal environment. Never falls back to
    /// a PATH search and never accepts a shell string.
    pub fn start_validation_command(
        &mut self,
        worktree_path: &Path,
        approval: &ValidationCommandApprovalRecord,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ValidationExecutionStartOutcome, PortFailure> {
        if let Err(rejection) = self.verify_bindings(worktree_path, approval) {
            return Ok(ValidationExecutionStartOutcome::BindingRejected(rejection));
        }

        let environment = build_environment(approval, &self.app_temp_dir);
        let spec = ProcessSpec {
            executable: PathBuf::from(&approval.approved_executable_path),
            arguments: approval
                .arguments
                .iter()
                .map(|argument| OsString::from(argument.as_str()))
                .collect(),
            working_directory: worktree_path.to_path_buf(),
            environment: Some(environment),
        };
        let deadline_cancellation = DeadlineCancellationSignal {
            caller: cancellation,
            deadline: Instant::now() + timeout_for_kind(approval.kind),
        };
        let mut observer = DiscardingObserver;
        let completion = self.streaming.run_streaming(
            &spec,
            None,
            MAX_STDOUT_BYTES,
            &deadline_cancellation,
            &mut observer,
        )?;
        Ok(ValidationExecutionStartOutcome::Completed(
            interpret_completion(completion, cancellation),
        ))
    }

    /// Re-verifies, from scratch, that `approval` still names one of this
    /// module's fixed Cargo candidates for its own `kind`, that the approved
    /// executable and its tool directory still resolve to the identity that
    /// was approved, that the executable's current canonical path is not
    /// inside `worktree_path`, and — for every environment binding
    /// `approval` itself carries (never a value supplied separately) — that
    /// it too still resolves to the identity durably recorded on `approval`.
    /// Every failure mode (an inspection error, including a reparse
    /// point/symlink that [`FilesystemIdentityPort`] already rejects, or a
    /// mismatch) is treated identically: reject, never spawn.
    fn verify_bindings(
        &mut self,
        worktree_path: &Path,
        approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), ValidationBindingRejection> {
        let Some(expected_arguments) = expected_cargo_arguments(approval.kind) else {
            return Err(ValidationBindingRejection::UnapprovedCommandKind);
        };
        let argv_matches = approval
            .arguments
            .iter()
            .map(String::as_str)
            .eq(expected_arguments.iter().copied());
        if approval.executable != CARGO_EXECUTABLE_NAME || !argv_matches {
            return Err(ValidationBindingRejection::UnapprovedCommandKind);
        }

        let executable_identity = self
            .filesystem
            .inspect_supported_file(Path::new(&approval.approved_executable_path))
            .map_err(|_error| ValidationBindingRejection::IdentityMismatch)?;
        if executable_identity.volume_serial_hex != approval.executable_volume_serial_hex
            || executable_identity.file_id_hex != approval.executable_file_id_hex
            || executable_identity.canonical_path.to_string_lossy()
                != approval.approved_executable_path
        {
            return Err(ValidationBindingRejection::IdentityMismatch);
        }

        let tool_directory_identity = self
            .filesystem
            .inspect_supported_directory(Path::new(&approval.tool_directory_path))
            .map_err(|_error| ValidationBindingRejection::IdentityMismatch)?;
        if tool_directory_identity.volume_serial_hex != approval.tool_directory_volume_serial_hex
            || tool_directory_identity.file_id_hex != approval.tool_directory_file_id_hex
        {
            return Err(ValidationBindingRejection::IdentityMismatch);
        }

        let worktree_identity = self
            .filesystem
            .inspect_supported_directory(worktree_path)
            .map_err(|_error| ValidationBindingRejection::IdentityMismatch)?;
        if executable_identity
            .canonical_path
            .starts_with(&worktree_identity.canonical_path)
        {
            return Err(ValidationBindingRejection::ExecutableInsideWorktree);
        }

        self.verify_environment_binding(
            approval.approved_cargo_home_path.as_deref(),
            approval.cargo_home_volume_serial_hex.as_deref(),
            approval.cargo_home_file_id_hex.as_deref(),
        )?;
        self.verify_environment_binding(
            approval.approved_rustup_home_path.as_deref(),
            approval.rustup_home_volume_serial_hex.as_deref(),
            approval.rustup_home_file_id_hex.as_deref(),
        )?;
        Ok(())
    }

    /// Re-verifies one optional `CARGO_HOME`/`RUSTUP_HOME` trio durably
    /// recorded on the approval. `(None, None, None)` (no approved override)
    /// always passes. A fully-populated trio must still resolve, via
    /// [`FilesystemIdentityPort::inspect_supported_directory`], to the exact
    /// same canonical path and stable identity that was approved. Any other
    /// combination (a partially-populated trio, which the SQL `CHECK` in
    /// `0012_validation_command_environment_binding.sql` should already
    /// prevent) is rejected rather than silently treated as "no override" —
    /// this adapter never trusts a value it cannot fully verify.
    fn verify_environment_binding(
        &mut self,
        approved_path: Option<&str>,
        approved_volume_serial_hex: Option<&str>,
        approved_file_id_hex: Option<&str>,
    ) -> Result<(), ValidationBindingRejection> {
        match (
            approved_path,
            approved_volume_serial_hex,
            approved_file_id_hex,
        ) {
            (None, None, None) => Ok(()),
            (Some(path), Some(volume_serial_hex), Some(file_id_hex)) => {
                let current = self
                    .filesystem
                    .inspect_supported_directory(Path::new(path))
                    .map_err(|_error| ValidationBindingRejection::IdentityMismatch)?;
                if current.volume_serial_hex != volume_serial_hex
                    || current.file_id_hex != file_id_hex
                    || current.canonical_path.to_string_lossy() != path
                {
                    return Err(ValidationBindingRejection::IdentityMismatch);
                }
                Ok(())
            }
            _ => Err(ValidationBindingRejection::IdentityMismatch),
        }
    }
}

impl<S, F> ValidationCommandExecutor for CargoValidationAdapter<S, F>
where
    S: StreamingProcessRunner,
    F: FilesystemIdentityPort,
{
    fn start_validation_command(
        &mut self,
        worktree_path: &Path,
        approval: &ValidationCommandApprovalRecord,
        cancellation: &dyn CancellationSignal,
    ) -> Result<ValidationExecutionStartOutcome, PortFailure> {
        CargoValidationAdapter::start_validation_command(
            self,
            worktree_path,
            approval,
            cancellation,
        )
    }
}

/// Composes a caller-supplied [`CancellationSignal`] with a kind-specific
/// wall-clock deadline so [`StreamingProcessRunner::run_streaming`]'s
/// existing cooperative cancellation polling also enforces the timeout,
/// without any change to that port or an extra timer thread. Mirrors
/// `chatoms_infrastructure::claude_implementation`'s identically-named type.
struct DeadlineCancellationSignal<'a> {
    caller: &'a dyn CancellationSignal,
    deadline: Instant,
}

impl CancellationSignal for DeadlineCancellationSignal<'_> {
    fn is_cancelled(&self) -> bool {
        self.caller.is_cancelled() || Instant::now() >= self.deadline
    }
}

/// A [`StreamingProcessObserver`] that discards every stdout chunk and
/// ignores every lifecycle event. This Unit performs no content
/// classification beyond the process's own exit code, so raw stdout is never
/// buffered, inspected, or returned anywhere — it exists only so
/// `run_streaming`'s own `max_stdout_bytes` bound still forces termination
/// of a pathologically chatty command.
struct DiscardingObserver;

impl StreamingProcessObserver for DiscardingObserver {
    fn on_stdout_chunk(&mut self, _chunk: &[u8]) {}

    fn on_event(&mut self, _event: ProcessLifecycleEvent) {}
}

const fn timeout_for_kind(kind: ValidationCommandKind) -> Duration {
    match kind {
        ValidationCommandKind::Format
        | ValidationCommandKind::Lint
        | ValidationCommandKind::Typecheck => SHORT_KIND_TIMEOUT,
        ValidationCommandKind::Test | ValidationCommandKind::Build => LONG_KIND_TIMEOUT,
    }
}

/// This Unit's own fixed Cargo vocabulary, deliberately re-stated rather
/// than imported from `crate::validation_discovery::cargo_candidates` (which
/// takes a worktree path and additionally checks `Cargo.toml` presence) —
/// duplicated on purpose so this Unit's diff never touches that
/// already-approved module, the same choice
/// `crate::claude_implementation` made relative to `crate::claude_planning`.
/// Returns `None` for `Typecheck` (Cargo discovery never proposes it) and
/// for any kind this adapter does not support executing.
fn expected_cargo_arguments(kind: ValidationCommandKind) -> Option<&'static [&'static str]> {
    match kind {
        ValidationCommandKind::Format => Some(&["fmt", "--all", "--check"]),
        ValidationCommandKind::Lint => {
            Some(&["clippy", "--workspace", "--all-targets", "--all-features"])
        }
        ValidationCommandKind::Test => Some(&["test", "--workspace"]),
        ValidationCommandKind::Build => Some(&["build", "--workspace"]),
        ValidationCommandKind::Typecheck => None,
    }
}

/// Builds the fully-specified controlled environment: `PATH` set to the
/// approved tool directory only, the app-owned `TEMP`/`TMP`, the current
/// process's own `SystemRoot` (an OS-set constant, not attacker-influenced,
/// and required for many Windows toolchains to resolve system DLLs), and —
/// only when `approval` itself carries a binding — `CARGO_HOME`/`RUSTUP_HOME`,
/// set to exactly the path [`CargoValidationAdapter::verify_environment_binding`]
/// just re-verified. Every other inherited variable is dropped: the caller
/// passes this as `ProcessSpec.environment`, which
/// `StdProcessRunner::run_streaming` honors by calling `env_clear()` first.
fn build_environment(
    approval: &ValidationCommandApprovalRecord,
    app_temp_dir: &Path,
) -> Vec<(OsString, OsString)> {
    let mut vars = vec![
        (
            OsString::from("PATH"),
            OsString::from(approval.tool_directory_path.as_str()),
        ),
        (OsString::from("TEMP"), app_temp_dir.as_os_str().to_owned()),
        (OsString::from("TMP"), app_temp_dir.as_os_str().to_owned()),
    ];
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        vars.push((OsString::from("SystemRoot"), system_root));
    }
    if let Some(cargo_home) = &approval.approved_cargo_home_path {
        vars.push((
            OsString::from("CARGO_HOME"),
            OsString::from(cargo_home.as_str()),
        ));
    }
    if let Some(rustup_home) = &approval.approved_rustup_home_path {
        vars.push((
            OsString::from("RUSTUP_HOME"),
            OsString::from(rustup_home.as_str()),
        ));
    }
    vars
}

/// Reduces a completed streaming invocation to the fail-closed
/// [`ValidationExecutionOutcome`] vocabulary. A `StreamingOutcome::Cancelled`
/// whose *caller-supplied* `cancellation` signal was never actually flipped
/// can only have been caused by [`DeadlineCancellationSignal`]'s own
/// deadline branch, so it is reported as `TimedOut` rather than `Cancelled`
/// — this needs no additional port support because it only inspects the
/// original signal `start_validation_command` was given, not the composed
/// one passed to `run_streaming`.
fn interpret_completion(
    completion: StreamingProcessCompletion,
    cancellation: &dyn CancellationSignal,
) -> ValidationExecutionOutcome {
    match completion.outcome {
        StreamingOutcome::StdoutBoundExceeded => ValidationExecutionOutcome::StdoutBoundExceeded,
        StreamingOutcome::Uncertain => ValidationExecutionOutcome::Uncertain,
        StreamingOutcome::Cancelled => {
            if cancellation.is_cancelled() {
                ValidationExecutionOutcome::Cancelled
            } else {
                ValidationExecutionOutcome::TimedOut
            }
        }
        StreamingOutcome::Completed => match completion.exit_code {
            Some(0) => ValidationExecutionOutcome::Success,
            Some(exit_code) => ValidationExecutionOutcome::ExitFailure { exit_code },
            None => ValidationExecutionOutcome::Uncertain,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chatoms_domain::TaskId;
    use chatoms_ports::{
        error::FailureCategory, filesystem::DirectoryIdentity, process::AtomicCancellationSignal,
    };
    use std::sync::{Arc, Mutex};

    fn approval_for(
        kind: ValidationCommandKind,
        arguments: &[&str],
    ) -> ValidationCommandApprovalRecord {
        ValidationCommandApprovalRecord {
            task_id: TaskId::new(),
            approved_task_version: 3,
            kind,
            executable: CARGO_EXECUTABLE_NAME.to_owned(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            approved_executable_path: "C:/tools/cargo/bin/cargo.exe".to_owned(),
            executable_volume_serial_hex: "0000000000000002".to_owned(),
            executable_file_id_hex: "00000000000000000000000000000002".to_owned(),
            tool_directory_path: "C:/tools/cargo/bin".to_owned(),
            tool_directory_volume_serial_hex: "0000000000000001".to_owned(),
            tool_directory_file_id_hex: "00000000000000000000000000000001".to_owned(),
            approved_cargo_home_path: None,
            cargo_home_volume_serial_hex: None,
            cargo_home_file_id_hex: None,
            approved_rustup_home_path: None,
            rustup_home_volume_serial_hex: None,
            rustup_home_file_id_hex: None,
            approved_at_ms: 30,
        }
    }

    /// Sets `approval`'s `CARGO_HOME`/`RUSTUP_HOME` fields to
    /// [`cargo_home_binding`]/[`rustup_home_binding`]'s path and identity, so
    /// a test can approve both homes without repeating every field inline.
    fn with_home_bindings(
        mut approval: ValidationCommandApprovalRecord,
    ) -> ValidationCommandApprovalRecord {
        let cargo_home = cargo_home_binding();
        let rustup_home = rustup_home_binding();
        approval.approved_cargo_home_path =
            Some(cargo_home.canonical_path.to_string_lossy().into_owned());
        approval.cargo_home_volume_serial_hex = Some(cargo_home.volume_serial_hex);
        approval.cargo_home_file_id_hex = Some(cargo_home.file_id_hex);
        approval.approved_rustup_home_path =
            Some(rustup_home.canonical_path.to_string_lossy().into_owned());
        approval.rustup_home_volume_serial_hex = Some(rustup_home.volume_serial_hex);
        approval.rustup_home_file_id_hex = Some(rustup_home.file_id_hex);
        approval
    }

    fn test_approval() -> ValidationCommandApprovalRecord {
        approval_for(ValidationCommandKind::Test, &["test", "--workspace"])
    }

    fn worktree_path() -> PathBuf {
        PathBuf::from("C:/managed/task")
    }

    fn cargo_home_binding() -> DirectoryIdentity {
        DirectoryIdentity {
            canonical_path: PathBuf::from("C:/tools/cargo-home"),
            volume_serial_hex: "0000000000000003".to_owned(),
            file_id_hex: "00000000000000000000000000000003".to_owned(),
        }
    }

    fn rustup_home_binding() -> DirectoryIdentity {
        DirectoryIdentity {
            canonical_path: PathBuf::from("C:/tools/rustup-home"),
            volume_serial_hex: "0000000000000004".to_owned(),
            file_id_hex: "00000000000000000000000000000004".to_owned(),
        }
    }

    /// Echoes back the queried path as `canonical_path` and returns a fixed
    /// identity per registered path, so tests can control mismatches by
    /// mutating the map between calls. An unregistered path (or one flagged
    /// `fail`) is a hard `NotFound` error, matching how
    /// `FilesystemIdentityPort::inspect_supported_file`/`_directory` already
    /// fail closed for anything unrecognized.
    #[derive(Clone, Default)]
    struct FakeFilesystemIdentity {
        directories: std::collections::HashMap<PathBuf, DirectoryIdentity>,
        files: std::collections::HashMap<PathBuf, DirectoryIdentity>,
        observed: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl FakeFilesystemIdentity {
        fn with_valid_bindings() -> Self {
            let mut fs = Self::default();
            fs.files.insert(
                PathBuf::from("C:/tools/cargo/bin/cargo.exe"),
                DirectoryIdentity {
                    canonical_path: PathBuf::from("C:/tools/cargo/bin/cargo.exe"),
                    volume_serial_hex: "0000000000000002".to_owned(),
                    file_id_hex: "00000000000000000000000000000002".to_owned(),
                },
            );
            fs.directories.insert(
                PathBuf::from("C:/tools/cargo/bin"),
                DirectoryIdentity {
                    canonical_path: PathBuf::from("C:/tools/cargo/bin"),
                    volume_serial_hex: "0000000000000001".to_owned(),
                    file_id_hex: "00000000000000000000000000000001".to_owned(),
                },
            );
            fs.directories.insert(
                worktree_path(),
                DirectoryIdentity {
                    canonical_path: worktree_path(),
                    volume_serial_hex: "0000000000000009".to_owned(),
                    file_id_hex: "00000000000000000000000000000009".to_owned(),
                },
            );
            fs.directories.insert(
                cargo_home_binding().canonical_path.clone(),
                cargo_home_binding(),
            );
            fs.directories.insert(
                rustup_home_binding().canonical_path.clone(),
                rustup_home_binding(),
            );
            fs
        }
    }

    impl FilesystemIdentityPort for FakeFilesystemIdentity {
        fn inspect_supported_directory(
            &mut self,
            path: &Path,
        ) -> Result<DirectoryIdentity, PortFailure> {
            self.observed
                .lock()
                .expect("observed lock")
                .push(path.to_path_buf());
            self.directories
                .get(path)
                .cloned()
                .ok_or_else(|| PortFailure::new(FailureCategory::NotFound))
        }

        fn verify_local_tree(&mut self, _root: &Path) -> Result<(), PortFailure> {
            Ok(())
        }

        fn acquire_guard(
            &mut self,
            _path: &Path,
            _expected: &DirectoryIdentity,
        ) -> Result<Box<dyn chatoms_ports::filesystem::DirectoryIdentityGuard>, PortFailure>
        {
            Err(PortFailure::new(FailureCategory::Unsupported))
        }

        fn inspect_supported_file(
            &mut self,
            path: &Path,
        ) -> Result<DirectoryIdentity, PortFailure> {
            self.observed
                .lock()
                .expect("observed lock")
                .push(path.to_path_buf());
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| PortFailure::new(FailureCategory::NotFound))
        }
    }

    type ObservedRun = (ProcessSpec, usize);

    #[derive(Clone, Default)]
    struct FakeStreamingRunner {
        observed: Arc<Mutex<Vec<ObservedRun>>>,
        scripted: Option<StreamingProcessCompletion>,
    }

    impl StreamingProcessRunner for FakeStreamingRunner {
        fn run_streaming(
            &mut self,
            spec: &ProcessSpec,
            _stdin: Option<&[u8]>,
            max_stdout_bytes: usize,
            _cancellation: &dyn CancellationSignal,
            _observer: &mut dyn StreamingProcessObserver,
        ) -> Result<StreamingProcessCompletion, PortFailure> {
            self.observed
                .lock()
                .expect("observed lock")
                .push((spec.clone(), max_stdout_bytes));
            self.scripted
                .ok_or_else(|| PortFailure::new(FailureCategory::Unsupported))
        }
    }

    fn completed(exit_code: i32) -> StreamingProcessCompletion {
        StreamingProcessCompletion {
            outcome: StreamingOutcome::Completed,
            exit_code: Some(exit_code),
        }
    }

    fn never_cancelled() -> AtomicCancellationSignal {
        AtomicCancellationSignal::new()
    }

    fn make_adapter(
        filesystem: FakeFilesystemIdentity,
        streaming: FakeStreamingRunner,
    ) -> CargoValidationAdapter<FakeStreamingRunner, FakeFilesystemIdentity> {
        CargoValidationAdapter::new(streaming, filesystem, PathBuf::from("C:/managed/temp"))
    }

    fn start(
        adapter: &mut CargoValidationAdapter<FakeStreamingRunner, FakeFilesystemIdentity>,
        approval: &ValidationCommandApprovalRecord,
    ) -> ValidationExecutionStartOutcome {
        adapter
            .start_validation_command(&worktree_path(), approval, &never_cancelled())
            .expect("start_validation_command returns a typed outcome")
    }

    fn completed_outcome(outcome: &ValidationExecutionStartOutcome) -> ValidationExecutionOutcome {
        match outcome {
            ValidationExecutionStartOutcome::Completed(result) => *result,
            other => panic!("expected a completed attempt, got {other:?}"),
        }
    }

    #[test]
    fn spawns_the_approved_absolute_executable_with_the_exact_approved_argv_and_worktree_cwd() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::Success
        );
        let runs = observed.lock().expect("observed lock");
        assert_eq!(runs.len(), 1);
        let (spec, max_bytes) = &runs[0];
        assert_eq!(spec.executable, Path::new("C:/tools/cargo/bin/cargo.exe"));
        assert_eq!(
            spec.arguments,
            vec![OsString::from("test"), OsString::from("--workspace"),]
        );
        assert_eq!(spec.working_directory, worktree_path());
        assert_eq!(*max_bytes, MAX_STDOUT_BYTES);
    }

    #[test]
    fn controlled_environment_contains_only_path_temp_tmp_systemroot_and_no_cargo_home_when_unapproved()
     {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        start(&mut adapter, &test_approval());

        let runs = observed.lock().expect("observed lock");
        let vars = runs[0]
            .0
            .environment
            .as_ref()
            .expect("environment must be Some so the runner env_clear's the child");
        let names: Vec<String> = vars
            .iter()
            .map(|(key, _value)| key.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"PATH".to_owned()));
        assert!(names.contains(&"TEMP".to_owned()));
        assert!(names.contains(&"TMP".to_owned()));
        assert!(
            !names.contains(&"CARGO_HOME".to_owned()),
            "CARGO_HOME must be omitted, never inherited, when no binding was approved"
        );
        assert!(
            !names.contains(&"RUSTUP_HOME".to_owned()),
            "RUSTUP_HOME must be omitted, never inherited, when no binding was approved"
        );
        let path_value = vars
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .expect("PATH must be present");
        assert_eq!(
            path_value, "C:/tools/cargo/bin",
            "PATH must contain the approved tool directory only"
        );
    }

    #[test]
    fn approved_cargo_home_and_rustup_home_are_re_verified_and_included_when_valid() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &with_home_bindings(test_approval()));

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::Success
        );
        let runs = observed.lock().expect("observed lock");
        let vars = runs[0].0.environment.as_ref().expect("environment set");
        let get = |name: &str| {
            vars.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(get("CARGO_HOME"), Some("C:/tools/cargo-home".to_owned()));
        assert_eq!(get("RUSTUP_HOME"), Some("C:/tools/rustup-home".to_owned()));
    }

    #[test]
    fn a_cargo_home_identity_mismatch_rejects_before_any_spawn() {
        let mut filesystem = FakeFilesystemIdentity::with_valid_bindings();
        // The directory at the approved CARGO_HOME path has since been
        // replaced by something else.
        filesystem.directories.insert(
            cargo_home_binding().canonical_path.clone(),
            DirectoryIdentity {
                canonical_path: cargo_home_binding().canonical_path,
                volume_serial_hex: "0000000000000099".to_owned(),
                file_id_hex: "00000000000000000000000000000099".to_owned(),
            },
        );
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(filesystem, streaming);
        let cargo_home = cargo_home_binding();
        let mut approval = test_approval();
        approval.approved_cargo_home_path =
            Some(cargo_home.canonical_path.to_string_lossy().into_owned());
        approval.cargo_home_volume_serial_hex = Some(cargo_home.volume_serial_hex);
        approval.cargo_home_file_id_hex = Some(cargo_home.file_id_hex);

        let outcome = start(&mut adapter, &approval);

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::IdentityMismatch
            )
        );
        assert!(
            observed.lock().expect("observed lock").is_empty(),
            "no subprocess may be started on a CARGO_HOME identity mismatch"
        );
    }

    #[test]
    fn executable_identity_mismatch_rejects_before_any_spawn() {
        let mut filesystem = FakeFilesystemIdentity::with_valid_bindings();
        filesystem.files.insert(
            PathBuf::from("C:/tools/cargo/bin/cargo.exe"),
            DirectoryIdentity {
                canonical_path: PathBuf::from("C:/tools/cargo/bin/cargo.exe"),
                volume_serial_hex: "0000000000000099".to_owned(),
                file_id_hex: "00000000000000000000000000000099".to_owned(),
            },
        );
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(filesystem, streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::IdentityMismatch
            )
        );
        assert!(observed.lock().expect("observed lock").is_empty());
    }

    #[test]
    fn a_reparse_point_or_otherwise_uninspectable_executable_rejects_before_any_spawn() {
        // The fake filesystem port treats an unregistered path exactly like
        // a real FilesystemIdentityPort implementation treats a reparse
        // point/symlink: a hard inspection error, never a usable identity.
        let filesystem = FakeFilesystemIdentity::default();
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(filesystem, streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::IdentityMismatch
            )
        );
        assert!(observed.lock().expect("observed lock").is_empty());
    }

    #[test]
    fn an_executable_resolving_inside_the_worktree_rejects_before_any_spawn() {
        let mut filesystem = FakeFilesystemIdentity::with_valid_bindings();
        let inside_worktree = worktree_path().join("vendored").join("cargo.exe");
        filesystem.files.insert(
            inside_worktree.clone(),
            DirectoryIdentity {
                canonical_path: inside_worktree.clone(),
                volume_serial_hex: "0000000000000002".to_owned(),
                file_id_hex: "00000000000000000000000000000002".to_owned(),
            },
        );
        let mut approval = test_approval();
        approval.approved_executable_path = inside_worktree.to_string_lossy().into_owned();
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(filesystem, streaming);

        let outcome = start(&mut adapter, &approval);

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::ExecutableInsideWorktree
            )
        );
        assert!(observed.lock().expect("observed lock").is_empty());
    }

    #[test]
    fn an_approval_whose_argv_does_not_match_the_fixed_cargo_vocabulary_is_rejected() {
        let approval = approval_for(
            ValidationCommandKind::Test,
            &["test", "--workspace", "--release"],
        );
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &approval);

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::UnapprovedCommandKind
            )
        );
        assert!(observed.lock().expect("observed lock").is_empty());
    }

    #[test]
    fn an_approval_for_a_non_cargo_executable_is_rejected_even_with_matching_argv() {
        let mut approval = approval_for(ValidationCommandKind::Test, &["test", "--workspace"]);
        approval.executable = "npm".to_owned();
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &approval);

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::UnapprovedCommandKind
            )
        );
        assert!(observed.lock().expect("observed lock").is_empty());
    }

    #[test]
    fn typecheck_has_no_fixed_cargo_vocabulary_and_is_always_rejected() {
        let approval = approval_for(ValidationCommandKind::Typecheck, &["check"]);
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &approval);

        assert_eq!(
            outcome,
            ValidationExecutionStartOutcome::BindingRejected(
                ValidationBindingRejection::UnapprovedCommandKind
            )
        );
    }

    #[test]
    fn format_and_lint_and_typecheck_kinds_use_the_short_timeout_and_test_and_build_use_the_long_one()
     {
        assert_eq!(
            timeout_for_kind(ValidationCommandKind::Format),
            SHORT_KIND_TIMEOUT
        );
        assert_eq!(
            timeout_for_kind(ValidationCommandKind::Lint),
            SHORT_KIND_TIMEOUT
        );
        assert_eq!(
            timeout_for_kind(ValidationCommandKind::Typecheck),
            SHORT_KIND_TIMEOUT
        );
        assert_eq!(
            timeout_for_kind(ValidationCommandKind::Test),
            LONG_KIND_TIMEOUT
        );
        assert_eq!(
            timeout_for_kind(ValidationCommandKind::Build),
            LONG_KIND_TIMEOUT
        );
    }

    #[test]
    fn nonzero_exit_is_classified_as_exit_failure_with_the_real_code() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(101)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::ExitFailure { exit_code: 101 }
        );
    }

    #[test]
    fn stdout_bound_exceeded_is_classified_accordingly() {
        let streaming = FakeStreamingRunner {
            scripted: Some(StreamingProcessCompletion {
                outcome: StreamingOutcome::StdoutBoundExceeded,
                exit_code: None,
            }),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::StdoutBoundExceeded
        );
    }

    #[test]
    fn uncertain_outcome_is_classified_accordingly() {
        let streaming = FakeStreamingRunner {
            scripted: Some(StreamingProcessCompletion {
                outcome: StreamingOutcome::Uncertain,
                exit_code: None,
            }),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::Uncertain
        );
    }

    #[test]
    fn a_genuine_caller_cancellation_is_reported_as_cancelled() {
        let streaming = FakeStreamingRunner {
            scripted: Some(StreamingProcessCompletion {
                outcome: StreamingOutcome::Cancelled,
                exit_code: None,
            }),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);
        let cancellation = AtomicCancellationSignal::new();
        cancellation.cancel();

        let outcome = adapter
            .start_validation_command(&worktree_path(), &test_approval(), &cancellation)
            .expect("typed outcome");

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::Cancelled
        );
    }

    #[test]
    fn a_cancelled_outcome_with_no_caller_cancellation_is_reported_as_timed_out() {
        // The runner reports a confirmed Cancelled exit, but the caller's
        // own signal was never flipped — this can only be
        // DeadlineCancellationSignal's own deadline branch firing.
        let streaming = FakeStreamingRunner {
            scripted: Some(StreamingProcessCompletion {
                outcome: StreamingOutcome::Cancelled,
                exit_code: None,
            }),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::TimedOut
        );
    }

    #[test]
    fn stdout_never_reaches_any_observer_the_adapter_exposes() {
        // DiscardingObserver has no way to leak content: it has no buffer
        // and start_validation_command never returns raw bytes. This test
        // documents that the returned outcome carries nothing beyond the
        // typed classification, regardless of what the runner "emitted".
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

        let outcome = start(&mut adapter, &test_approval());

        assert_eq!(
            completed_outcome(&outcome),
            ValidationExecutionOutcome::Success
        );
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains("stdout"));
    }

    #[test]
    fn run_streaming_failure_after_passed_bindings_still_surfaces_as_an_error() {
        let mut adapter = make_adapter(
            FakeFilesystemIdentity::with_valid_bindings(),
            FakeStreamingRunner {
                scripted: None,
                ..FakeStreamingRunner::default()
            },
        );

        adapter
            .start_validation_command(&worktree_path(), &test_approval(), &never_cancelled())
            .expect_err("a genuine spawn failure must not be silently swallowed");
    }

    #[test]
    fn format_lint_and_build_kinds_each_spawn_their_own_fixed_cargo_argv() {
        let cases: [(ValidationCommandKind, &[&str]); 3] = [
            (ValidationCommandKind::Format, &["fmt", "--all", "--check"]),
            (
                ValidationCommandKind::Lint,
                &["clippy", "--workspace", "--all-targets", "--all-features"],
            ),
            (ValidationCommandKind::Build, &["build", "--workspace"]),
        ];
        for (kind, arguments) in cases {
            let approval = approval_for(kind, arguments);
            let streaming = FakeStreamingRunner {
                scripted: Some(completed(0)),
                ..FakeStreamingRunner::default()
            };
            let observed = streaming.observed.clone();
            let mut adapter =
                make_adapter(FakeFilesystemIdentity::with_valid_bindings(), streaming);

            let outcome = start(&mut adapter, &approval);

            assert_eq!(
                completed_outcome(&outcome),
                ValidationExecutionOutcome::Success,
                "case: {kind:?}"
            );
            let runs = observed.lock().expect("observed lock");
            let expected_args: Vec<OsString> = arguments
                .iter()
                .map(|argument| OsString::from(*argument))
                .collect();
            assert_eq!(runs[0].0.arguments, expected_args, "case: {kind:?}");
        }
    }
}
