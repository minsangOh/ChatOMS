use std::{ffi::OsString, path::Path, path::PathBuf};

#[cfg(windows)]
use chatoms_platform::claude_trust::TrustedClaudeExecutable;
#[cfg(windows)]
use chatoms_platform::preflight::TrustedPreflightWorkingDirectory;
use chatoms_ports::{
    error::PortFailure,
    process::{ProcessOutcome, ProcessRunner},
    provider::{ProviderCapabilities, ProviderCapabilityPort, ProviderCapabilityStatus},
};

#[cfg(not(windows))]
struct TrustedClaudeExecutable;

#[cfg(not(windows))]
impl TrustedClaudeExecutable {
    fn verify(_path: &std::path::Path) -> Result<Self, ()> {
        Err(())
    }
}

#[cfg(not(windows))]
struct TrustedPreflightWorkingDirectory;

#[cfg(not(windows))]
impl TrustedPreflightWorkingDirectory {
    fn revalidate(&self) -> Result<(), ()> {
        Err(())
    }

    fn path(&self) -> &std::path::Path {
        std::path::Path::new(".")
    }
}

/// Outcome of a local, non-payload compatibility probe. Never carries the
/// captured stdout/stderr bytes that produced it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompatibilityCheck {
    Compatible,
    Incompatible,
}

/// Outcome of the login preflight. Never carries stdout/stderr; only the
/// exit code decides this.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoginCheck {
    LoggedIn,
    NotLoggedIn,
}

/// Required `--help` tokens for the confirmed read-only execution contract
/// (see docs/DECISIONS.md's "Claude 읽기 전용 계약"): the permission-mode
/// gate, its `plan` value, the tool-restriction flag, and the three
/// allowlisted tools.
const REQUIRED_HELP_TOKENS: [&str; 6] = [
    "--permission-mode",
    "plan",
    "--tools",
    "Read",
    "Glob",
    "Grep",
];

/// Reports Claude/Codex execution capability from a caller-supplied,
/// user-designated executable path. Never searches `PATH` or a fixed
/// candidate list; a missing path, an unverifiable path, or any signature
/// failure is `Unsupported`, never a partial or degraded success.
///
/// Claude is reported `Supported` only when all four gates pass, in order:
/// executable trust, provider preflight working directory revalidation,
/// local `--version`/`--help` compatibility, and `claude auth status` login
/// state. Trust and preflight directory revalidation are re-checked
/// immediately before every one of the (up to three) process spawns, since
/// either can be replaced between checks. Any gate failing or being
/// ambiguous is fail-closed to `Unsupported`, never a partial or degraded
/// success. stdout is interpreted only in-memory for the minimal
/// compatibility decision and immediately discarded; stderr is never read
/// on any of these paths (see docs/SECURITY_POLICY.md's "Phase 3 provider
/// 실행파일 신뢰 경계").
///
/// Codex has no verified executable trust boundary yet and is
/// unconditionally `Unsupported` until a separately approved implementation
/// exists.
pub struct StdProviderCapabilityAdapter<R> {
    claude_executable: Option<PathBuf>,
    preflight_dir: Option<TrustedPreflightWorkingDirectory>,
    process_runner: R,
}

impl<R> StdProviderCapabilityAdapter<R> {
    #[must_use]
    pub const fn new(
        claude_executable: Option<PathBuf>,
        preflight_dir: Option<TrustedPreflightWorkingDirectory>,
        process_runner: R,
    ) -> Self {
        Self {
            claude_executable,
            preflight_dir,
            process_runner,
        }
    }
}

impl<R> ProviderCapabilityPort for StdProviderCapabilityAdapter<R>
where
    R: ProcessRunner,
{
    fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure> {
        let claude = self.claude_capability();
        Ok(ProviderCapabilities {
            claude,
            codex: ProviderCapabilityStatus::Unsupported,
        })
    }
}

impl<R> StdProviderCapabilityAdapter<R>
where
    R: ProcessRunner,
{
    fn claude_capability(&mut self) -> ProviderCapabilityStatus {
        let Some(executable) = self.claude_executable.as_deref() else {
            return ProviderCapabilityStatus::Unsupported;
        };
        let Some(preflight_dir) = self.preflight_dir.as_ref() else {
            return ProviderCapabilityStatus::Unsupported;
        };

        // Fast, non-process gates first: no child process is spawned unless
        // trust and the preflight directory are both currently valid.
        if !trust_and_directory_are_valid(executable, preflight_dir) {
            return ProviderCapabilityStatus::Unsupported;
        }

        let working_directory = preflight_dir.path();

        if !trust_and_directory_are_valid(executable, preflight_dir)
            || probe_version(&mut self.process_runner, executable, working_directory)
                != CompatibilityCheck::Compatible
        {
            return ProviderCapabilityStatus::Unsupported;
        }

        if !trust_and_directory_are_valid(executable, preflight_dir)
            || probe_help(&mut self.process_runner, executable, working_directory)
                != CompatibilityCheck::Compatible
        {
            return ProviderCapabilityStatus::Unsupported;
        }

        if !trust_and_directory_are_valid(executable, preflight_dir)
            || probe_login(&mut self.process_runner, executable, working_directory)
                != LoginCheck::LoggedIn
        {
            return ProviderCapabilityStatus::Unsupported;
        }

        ProviderCapabilityStatus::Supported
    }
}

fn trust_and_directory_are_valid(
    executable: &Path,
    preflight_dir: &TrustedPreflightWorkingDirectory,
) -> bool {
    TrustedClaudeExecutable::verify(executable).is_ok() && preflight_dir.revalidate().is_ok()
}

/// Runs `claude --version` with no stdin in the trusted preflight working
/// directory. stdout is interpreted only for the minimal pattern below and
/// discarded when this function returns; stderr is never read.
fn probe_version(
    runner: &mut dyn ProcessRunner,
    executable: &Path,
    working_directory: &Path,
) -> CompatibilityCheck {
    let arguments = [OsString::from("--version")];
    match runner.run(executable, &arguments, working_directory, None) {
        Ok(completion)
            if completion.outcome == ProcessOutcome::Completed
                && completion.exit_code == Some(0)
                && version_stdout_is_compatible(&completion.stdout) =>
        {
            CompatibilityCheck::Compatible
        }
        _ => CompatibilityCheck::Incompatible,
    }
}

/// Runs `claude --help` with no stdin in the trusted preflight working
/// directory. stdout is interpreted only for the required-token check below
/// and discarded when this function returns; stderr is never read. `--help`
/// is not guaranteed to list every supported flag, so a missing token here
/// is a safe (if occasionally over-conservative) fail-closed result, never
/// a partial success.
fn probe_help(
    runner: &mut dyn ProcessRunner,
    executable: &Path,
    working_directory: &Path,
) -> CompatibilityCheck {
    let arguments = [OsString::from("--help")];
    match runner.run(executable, &arguments, working_directory, None) {
        Ok(completion)
            if completion.outcome == ProcessOutcome::Completed
                && completion.exit_code == Some(0)
                && help_stdout_is_compatible(&completion.stdout) =>
        {
            CompatibilityCheck::Compatible
        }
        _ => CompatibilityCheck::Incompatible,
    }
}

/// Runs `claude auth status` with no stdin in the trusted preflight working
/// directory. Only `outcome`/`exit_code` are read; stdout and stderr are
/// never bound, per docs/SECURITY_POLICY.md's rule that login state is
/// judged by exit code alone.
fn probe_login(
    runner: &mut dyn ProcessRunner,
    executable: &Path,
    working_directory: &Path,
) -> LoginCheck {
    let arguments = [OsString::from("auth"), OsString::from("status")];
    match runner.run(executable, &arguments, working_directory, None) {
        Ok(completion)
            if completion.outcome == ProcessOutcome::Completed
                && completion.exit_code == Some(0) =>
        {
            LoginCheck::LoggedIn
        }
        _ => LoginCheck::NotLoggedIn,
    }
}

/// Confirmed official output shape (docs/SECURITY_POLICY.md's Phase 3
/// provider trust section, verified against code.claude.com's
/// troubleshoot-install page): a successful `claude --version` prints a
/// version number and `(Claude Code)`.
fn version_stdout_is_compatible(stdout: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stdout);
    text.contains("(Claude Code)") && contains_dotted_version_number(&text)
}

fn help_stdout_is_compatible(stdout: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stdout);
    REQUIRED_HELP_TOKENS
        .iter()
        .all(|token| text.contains(token))
}

/// Minimal, dependency-free scan for a `<digits>.<digits>` pattern, matching
/// the leading segments of a version string such as `2.1.211`.
fn contains_dotted_version_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let mut cursor = index;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b'.' {
                let fraction_start = cursor + 1;
                let mut fraction_end = fraction_start;
                while fraction_end < bytes.len() && bytes[fraction_end].is_ascii_digit() {
                    fraction_end += 1;
                }
                if fraction_end > fraction_start {
                    return true;
                }
            }
            index = cursor.max(index + 1);
        } else {
            index += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chatoms_ports::{error::FailureCategory, process::ProcessCompletion};
    use std::ffi::OsStr;

    type ObservedCall = (PathBuf, Vec<OsString>, PathBuf, Option<Vec<u8>>);

    #[derive(Default)]
    struct ScriptedRunner {
        version: Option<Result<ProcessCompletion, PortFailure>>,
        help: Option<Result<ProcessCompletion, PortFailure>>,
        login: Option<Result<ProcessCompletion, PortFailure>>,
        observed: Vec<ObservedCall>,
    }

    impl ProcessRunner for ScriptedRunner {
        fn run(
            &mut self,
            executable: &Path,
            arguments: &[OsString],
            working_directory: &Path,
            stdin: Option<&[u8]>,
        ) -> Result<ProcessCompletion, PortFailure> {
            self.observed.push((
                executable.to_path_buf(),
                arguments.to_vec(),
                working_directory.to_path_buf(),
                stdin.map(<[u8]>::to_vec),
            ));
            let first = arguments.first().map(|value| value.as_os_str());
            let scripted = if first == Some(OsStr::new("--version")) {
                self.version.clone()
            } else if first == Some(OsStr::new("--help")) {
                self.help.clone()
            } else if first == Some(OsStr::new("auth")) {
                self.login.clone()
            } else {
                None
            };
            scripted.unwrap_or_else(|| Err(PortFailure::new(FailureCategory::Unsupported)))
        }
    }

    fn completed(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> ProcessCompletion {
        ProcessCompletion {
            outcome: ProcessOutcome::Completed,
            exit_code: Some(exit_code),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    fn uncertain() -> ProcessCompletion {
        ProcessCompletion {
            outcome: ProcessOutcome::Uncertain,
            exit_code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    const COMPATIBLE_VERSION_STDOUT: &[u8] = b"2.1.211 (Claude Code)\n";
    const COMPATIBLE_HELP_STDOUT: &[u8] =
        b"--permission-mode <mode> (default|acceptEdits|plan|...)\n--tools <tools>\nRead, Glob, Grep";

    #[test]
    fn probe_version_compatible_on_completed_zero_exit_and_expected_pattern() {
        let mut runner = ScriptedRunner {
            version: Some(Ok(completed(
                0,
                COMPATIBLE_VERSION_STDOUT,
                b"stderr-must-be-ignored",
            ))),
            ..Default::default()
        };
        assert_eq!(
            probe_version(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Compatible
        );
    }

    #[test]
    fn probe_version_incompatible_on_nonzero_exit() {
        let mut runner = ScriptedRunner {
            version: Some(Ok(completed(1, COMPATIBLE_VERSION_STDOUT, b""))),
            ..Default::default()
        };
        assert_eq!(
            probe_version(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_version_incompatible_on_uncertain_outcome() {
        let mut runner = ScriptedRunner {
            version: Some(Ok(uncertain())),
            ..Default::default()
        };
        assert_eq!(
            probe_version(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_version_incompatible_on_unrecognized_stdout() {
        let mut runner = ScriptedRunner {
            version: Some(Ok(completed(0, b"not a recognizable version banner", b""))),
            ..Default::default()
        };
        assert_eq!(
            probe_version(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_version_incompatible_on_runner_error() {
        let mut runner = ScriptedRunner {
            version: Some(Err(PortFailure::new(FailureCategory::NotFound))),
            ..Default::default()
        };
        assert_eq!(
            probe_version(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_help_compatible_on_completed_zero_exit_and_all_required_tokens() {
        let mut runner = ScriptedRunner {
            help: Some(Ok(completed(
                0,
                COMPATIBLE_HELP_STDOUT,
                b"stderr-must-be-ignored",
            ))),
            ..Default::default()
        };
        assert_eq!(
            probe_help(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Compatible
        );
    }

    #[test]
    fn probe_help_incompatible_when_a_required_token_is_missing() {
        let mut runner = ScriptedRunner {
            help: Some(Ok(completed(
                0,
                b"--permission-mode <mode>\n--tools <tools>\nRead, Glob",
                b"",
            ))),
            ..Default::default()
        };
        assert_eq!(
            probe_help(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible,
            "Grep is missing from the scripted --help output"
        );
    }

    #[test]
    fn probe_help_incompatible_on_nonzero_exit() {
        let mut runner = ScriptedRunner {
            help: Some(Ok(completed(1, COMPATIBLE_HELP_STDOUT, b""))),
            ..Default::default()
        };
        assert_eq!(
            probe_help(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_help_incompatible_on_uncertain_outcome() {
        let mut runner = ScriptedRunner {
            help: Some(Ok(uncertain())),
            ..Default::default()
        };
        assert_eq!(
            probe_help(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_help_incompatible_on_runner_error() {
        let mut runner = ScriptedRunner {
            help: Some(Err(PortFailure::new(FailureCategory::Unsupported))),
            ..Default::default()
        };
        assert_eq!(
            probe_help(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            CompatibilityCheck::Incompatible
        );
    }

    #[test]
    fn probe_login_logged_in_only_on_completed_zero_exit() {
        let mut runner = ScriptedRunner {
            login: Some(Ok(completed(
                0,
                b"stdout-must-be-ignored",
                b"stderr-must-be-ignored",
            ))),
            ..Default::default()
        };
        assert_eq!(
            probe_login(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            LoginCheck::LoggedIn
        );
    }

    #[test]
    fn probe_login_not_logged_in_on_exit_code_one() {
        let mut runner = ScriptedRunner {
            login: Some(Ok(completed(1, b"", b""))),
            ..Default::default()
        };
        assert_eq!(
            probe_login(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            LoginCheck::NotLoggedIn
        );
    }

    #[test]
    fn probe_login_not_logged_in_on_other_exit_codes() {
        let mut runner = ScriptedRunner {
            login: Some(Ok(completed(2, b"", b""))),
            ..Default::default()
        };
        assert_eq!(
            probe_login(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            LoginCheck::NotLoggedIn
        );
    }

    #[test]
    fn probe_login_not_logged_in_on_uncertain_outcome() {
        let mut runner = ScriptedRunner {
            login: Some(Ok(uncertain())),
            ..Default::default()
        };
        assert_eq!(
            probe_login(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            LoginCheck::NotLoggedIn
        );
    }

    #[test]
    fn probe_login_not_logged_in_on_runner_error() {
        let mut runner = ScriptedRunner {
            login: Some(Err(PortFailure::new(FailureCategory::PermissionDenied))),
            ..Default::default()
        };
        assert_eq!(
            probe_login(&mut runner, Path::new("claude.exe"), Path::new("preflight")),
            LoginCheck::NotLoggedIn
        );
    }

    #[test]
    fn each_probe_uses_the_given_executable_working_directory_and_no_stdin() {
        let executable = Path::new("C:/trusted/claude.exe");
        let working_directory = Path::new("C:/preflight/provider-preflight");
        let mut runner = ScriptedRunner {
            version: Some(Ok(completed(0, COMPATIBLE_VERSION_STDOUT, b""))),
            help: Some(Ok(completed(0, COMPATIBLE_HELP_STDOUT, b""))),
            login: Some(Ok(completed(0, b"", b""))),
            ..Default::default()
        };

        probe_version(&mut runner, executable, working_directory);
        probe_help(&mut runner, executable, working_directory);
        probe_login(&mut runner, executable, working_directory);

        assert_eq!(runner.observed.len(), 3);
        for (observed_executable, _, observed_directory, stdin) in &runner.observed {
            assert_eq!(observed_executable, executable);
            assert_eq!(observed_directory, working_directory);
            assert_eq!(stdin, &None, "no probe may write to stdin");
        }
        assert_eq!(runner.observed[0].1, vec![OsString::from("--version")]);
        assert_eq!(runner.observed[1].1, vec![OsString::from("--help")]);
        assert_eq!(
            runner.observed[2].1,
            vec![OsString::from("auth"), OsString::from("status")]
        );
    }

    #[test]
    fn canary_in_stdout_and_stderr_never_appears_in_any_probe_result() {
        const CANARY: &str = "super-secret-canary-token-should-never-leak";
        let version_stdout = format!("2.1.211 (Claude Code) {CANARY}");
        let help_stdout = format!("--permission-mode plan --tools Read Glob Grep {CANARY}");
        let mut runner = ScriptedRunner {
            version: Some(Ok(completed(
                0,
                version_stdout.as_bytes(),
                CANARY.as_bytes(),
            ))),
            help: Some(Ok(completed(0, help_stdout.as_bytes(), CANARY.as_bytes()))),
            login: Some(Ok(completed(0, CANARY.as_bytes(), CANARY.as_bytes()))),
            ..Default::default()
        };

        let version = probe_version(&mut runner, Path::new("claude.exe"), Path::new("preflight"));
        let help = probe_help(&mut runner, Path::new("claude.exe"), Path::new("preflight"));
        let login = probe_login(&mut runner, Path::new("claude.exe"), Path::new("preflight"));

        let rendered = format!("{version:?} {help:?} {login:?}");
        assert!(
            !rendered.contains(CANARY),
            "probe results must never carry the raw stdout/stderr that produced them"
        );
        assert_eq!(version, CompatibilityCheck::Compatible);
        assert_eq!(help, CompatibilityCheck::Compatible);
        assert_eq!(login, LoginCheck::LoggedIn);
    }

    #[test]
    fn version_stdout_pattern_requires_both_marker_and_dotted_number() {
        assert!(version_stdout_is_compatible(b"2.1.211 (Claude Code)"));
        assert!(!version_stdout_is_compatible(b"(Claude Code)"));
        assert!(!version_stdout_is_compatible(b"2.1.211"));
        assert!(!version_stdout_is_compatible(b""));
    }

    #[test]
    fn help_stdout_requires_every_token() {
        assert!(help_stdout_is_compatible(COMPATIBLE_HELP_STDOUT));
        assert!(!help_stdout_is_compatible(
            b"--permission-mode plan --tools Read Glob"
        ));
        assert!(!help_stdout_is_compatible(b""));
    }

    #[test]
    fn contains_dotted_version_number_matches_leading_digits_only() {
        assert!(contains_dotted_version_number("2.1.211"));
        assert!(contains_dotted_version_number("prefix 2.1 suffix"));
        assert!(!contains_dotted_version_number("no version here"));
        assert!(!contains_dotted_version_number("trailing dot 2."));
    }
}
