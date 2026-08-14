//! Claude Planning execution adapter.
//!
//! Wires the approved read-only Claude Planning contract (see
//! docs/DECISIONS.md's "Claude 읽기 전용 계약") to the Unit 3
//! [`StreamingProcessRunner`] port. This module never runs the real
//! `claude` executable itself: it only builds the argv/CWD/stdin for a
//! spawn and delegates the actual process lifecycle to a caller-supplied
//! [`StreamingProcessRunner`] implementation (production code injects
//! [`crate::process::StdProcessRunner`]; tests inject a fake).
//!
//! `--output-format json` makes the CLI print exactly one JSON envelope at
//! the end of the run (confirmed against the official CLI/Agent SDK docs:
//! `-p --permission-mode plan` explores and prints the plan to stdout
//! without an interactive approval dialog, and print-mode's `json` format
//! is that single result object — never the incremental per-event stream
//! `stream-json` would produce). That single-envelope shape is what makes
//! "use only the final result, never a partial message" straightforward:
//! there is nothing else in the buffer to accidentally pick up.
//!
//! Three safety properties are structural, not just disciplined:
//!
//! * [`ClaudePlanningObserver`] has no raw-byte callback, so nothing
//!   implementing it can ever receive stdout content — only the
//!   content-free [`ProcessLifecycleEvent`] values already documented as
//!   safe to surface further.
//! * [`ClaudePlanningAdapter::start_planning`] re-runs the full Claude
//!   trust/compatibility/login/preflight gate (via the injected
//!   [`ProviderCapabilityPort`]) immediately before every spawn attempt; a
//!   cached "Supported" result from an earlier call (e.g. Unit 2's system
//!   status probe) is never trusted on its own.
//! * Raw stdout bytes are accumulated only inside a private relay local to
//!   [`ClaudePlanningAdapter::start_planning`]'s call frame, bounded by
//!   [`MAX_STDOUT_BYTES`], and are only ever fed to this module's own
//!   schema parser and [`crate::redaction::SecretRedactor`]. The masked,
//!   size-capped result string that comes back out is the only content
//!   [`ClaudePlanningResult`] ever carries.
//!
//! Unit 4e-8b brought Planning's argv hardening and stdin bound up to the
//! same level already approved for Claude Implementation/Review (confirmed
//! against the same official Claude Code CLI docs those adapters cite:
//! code.claude.com/docs/en/{permission-modes,cli-reference,headless,permissions}):
//!
//! * `--strict-mcp-config` passed with no `--mcp-config` value restricts the
//!   session to the (empty) server set `--mcp-config` would have supplied
//!   and ignores every other MCP source, so no MCP server loads — closing
//!   off a spawn side-effect that existed independently of the `--tools`
//!   allowlist.
//! * `--setting-sources project,local` (omitting `user`) stops
//!   `~/.claude/settings.json` — including any hooks it defines — from
//!   loading. Hooks run on session events, not on tool use, so they were
//!   never gated by `--tools Read,Glob,Grep` in the first place. Project/
//!   local sources are harmless here because the CWD is an app-owned
//!   preflight directory with no `.claude/` folder of its own. Managed
//!   (org-deployed) settings are not gated by this flag at all and can
//!   still apply; that residual is the same "회사 장비 정책 우선" principle
//!   `docs/SECURITY_POLICY.md` already accepts elsewhere, not a new gap.
//! * `--disable-slash-commands` additionally disables skills and custom
//!   commands for the session, closing one more customization surface.
//! * [`MAX_STDIN_BYTES`] bounds the composed stdin payload; exceeding it is
//!   a typed [`ClaudePlanningStartOutcome::StdinTooLarge`] fail-closed
//!   result checked before any spawn, never a truncated send.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use chatoms_ports::{
    error::PortFailure,
    process::{
        CancellationSignal, ProcessLifecycleEvent, ProcessSpec, StreamingOutcome,
        StreamingProcessCompletion, StreamingProcessObserver, StreamingProcessRunner,
    },
    provider::{ProviderCapabilityPort, ProviderCapabilityStatus},
    repository::PlanningResultOutcome,
};

use crate::redaction::SecretRedactor;

/// Built-in tools Claude Planning may use. Restricting to these three
/// (rather than relying on permission prompts alone) means no other
/// built-in tool exists in the session at all, matching the approved
/// read-only contract independently of permission-mode behavior.
const ALLOWED_TOOLS: &str = "Read,Glob,Grep";

/// Hard turn ceiling enforced by the CLI itself (`--max-turns`), not by
/// this adapter counting turns after the fact.
const MAX_TURNS: &str = "12";

/// `user` is deliberately omitted so `~/.claude/settings.json` (and any
/// hooks it defines) never loads — see the module doc for why hooks are not
/// already covered by the `--tools` allowlist.
const SETTING_SOURCES: &str = "project,local";

/// Fixed, non-parameterized instruction: the only positional prompt text
/// sent to the CLI. It never contains task-specific or user-supplied
/// content — the actual `TaskBrief` fields travel exclusively through
/// stdin (see [`format_stdin`]), never through argv.
const FIXED_INSTRUCTION: &str = "Follow the requirements, completion criteria, and prohibited \
    scope provided on stdin. Analyze the read-only worktree made available via --add-dir and \
    produce a plan only. Do not create, edit, or delete any file, and do not run any command \
    that would do so.";

/// Bound on how much stdout this adapter will ever let `run_streaming`
/// deliver before it treats the run as exceeding its output budget. Fixed
/// here, not caller-supplied, so nothing above this module can widen it.
/// A read-only plan response is text; a few hundred KiB is already
/// generous for that, so this leaves headroom without being unbounded.
const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;

/// Bound on the total stdin payload this adapter will ever send. Checked
/// before a spawn is attempted: exceeding it is a
/// [`ClaudePlanningStartOutcome::StdinTooLarge`] fail-closed result, never a
/// truncated send. Planning's stdin carries only the three
/// [`chatoms_domain::TaskBrief`] fields (unlike Claude Implementation's
/// stdin, which also carries a stored plan of up to 100,000 bytes), so this
/// is set well below [`crate::claude_implementation`]'s 512 KiB cap while
/// still leaving generous headroom for realistic `TaskBrief` field lengths,
/// and far under the CLI's own 10 MiB piped-stdin cap.
const MAX_STDIN_BYTES: usize = 256 * 1024;

/// The three fixed [`chatoms_domain::TaskBrief`] fields, borrowed rather
/// than owned so callers do not need to clone brief text just to start a
/// planning run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanningBrief<'a> {
    pub requirements: &'a str,
    pub completion_criteria: &'a str,
    pub prohibited_scope: &'a str,
}

/// Content-free notifications for a Claude Planning run. Deliberately has
/// no raw-byte callback: [`ClaudePlanningAdapter`] is the only code that
/// ever sees stdout bytes, and it never forwards them here.
pub trait ClaudePlanningObserver {
    fn on_event(&mut self, event: ProcessLifecycleEvent);
}

/// Result of attempting to start a Claude Planning invocation.
/// `PreflightRejected` means the fresh trust/compatibility/login/preflight
/// gate failed immediately before spawn. `StdinTooLarge` means the composed
/// stdin payload exceeded [`MAX_STDIN_BYTES`]. Neither starts a subprocess.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaudePlanningStartOutcome {
    Completed(ClaudePlanningResult),
    PreflightRejected,
    StdinTooLarge,
}

/// A Claude Planning attempt reduced to the safe, Task-state-machine-ready
/// vocabulary. `plan_text` is masked and size-bounded by
/// [`SecretRedactor::redact_text`] and is `Some` only when `outcome` is
/// `Completed`. This is the only type that ever crosses out of this
/// module carrying content derived from the child process's stdout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudePlanningResult {
    pub outcome: PlanningResultOutcome,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub plan_text: Option<String>,
}

/// Adapter that runs Claude Planning through a [`StreamingProcessRunner`],
/// gated by a fresh [`ProviderCapabilityPort`] check on every attempt.
///
/// `claude_executable` and `preflight_dir` are the same raw, caller-owned
/// paths the injected `capability` port was built from; this type never
/// re-derives or caches a "trusted" path of its own. Any staleness between
/// the two is a caller wiring bug, not something this adapter can detect,
/// which is why the fresh capability check — not a locally cached flag —
/// is what gates every spawn.
pub struct ClaudePlanningAdapter<C, S> {
    capability: C,
    streaming: S,
    claude_executable: PathBuf,
    preflight_dir: PathBuf,
    redactor: SecretRedactor,
}

impl<C, S> ClaudePlanningAdapter<C, S>
where
    C: ProviderCapabilityPort,
    S: StreamingProcessRunner,
{
    #[must_use]
    pub const fn new(
        capability: C,
        streaming: S,
        claude_executable: PathBuf,
        preflight_dir: PathBuf,
        redactor: SecretRedactor,
    ) -> Self {
        Self {
            capability,
            streaming,
            claude_executable,
            preflight_dir,
            redactor,
        }
    }

    /// Re-verifies Claude trust, compatibility, login, and preflight
    /// directory state fresh (never trusting an earlier cached result),
    /// and only then spawns the read-only Planning invocation. `worktree`
    /// is passed to the CLI as a read-access `--add-dir` argument; it is
    /// never used as the child's working directory. `observer` receives
    /// only content-free lifecycle events, never stdout bytes.
    pub fn start_planning(
        &mut self,
        worktree: &Path,
        brief: PlanningBrief<'_>,
        cancellation: &dyn CancellationSignal,
        observer: &mut dyn ClaudePlanningObserver,
    ) -> Result<ClaudePlanningStartOutcome, PortFailure> {
        let capabilities = self.capability.provider_capabilities()?;
        if capabilities.claude != ProviderCapabilityStatus::Supported {
            return Ok(ClaudePlanningStartOutcome::PreflightRejected);
        }

        let stdin = format_stdin(&brief);
        if stdin.len() > MAX_STDIN_BYTES {
            return Ok(ClaudePlanningStartOutcome::StdinTooLarge);
        }

        let spec = ProcessSpec {
            executable: self.claude_executable.clone(),
            arguments: planning_arguments(worktree),
            working_directory: self.preflight_dir.clone(),
            environment: None,
        };
        let mut relay = ResultCapturingRelay {
            inner: observer,
            buffer: Vec::new(),
        };
        let completion = self.streaming.run_streaming(
            &spec,
            Some(&stdin),
            MAX_STDOUT_BYTES,
            cancellation,
            &mut relay,
        )?;
        let result = interpret_completion(completion, &relay.buffer, &self.redactor);
        Ok(ClaudePlanningStartOutcome::Completed(result))
    }
}

/// Forwards only [`ProcessLifecycleEvent`] values to the caller's
/// [`ClaudePlanningObserver`], and accumulates stdout bytes into a private,
/// bounded buffer this module's own schema parser consumes after the run
/// completes. This is the only place in the adapter that ever touches raw
/// stdout bytes; the buffer is dropped at the end of `start_planning`'s
/// call frame and never itself returned to any caller.
struct ResultCapturingRelay<'a> {
    inner: &'a mut dyn ClaudePlanningObserver,
    buffer: Vec<u8>,
}

impl StreamingProcessObserver for ResultCapturingRelay<'_> {
    fn on_stdout_chunk(&mut self, chunk: &[u8]) {
        if self.buffer.len() >= MAX_STDOUT_BYTES {
            return;
        }
        let remaining = MAX_STDOUT_BYTES - self.buffer.len();
        let take = remaining.min(chunk.len());
        self.buffer.extend_from_slice(&chunk[..take]);
    }

    fn on_event(&mut self, event: ProcessLifecycleEvent) {
        self.inner.on_event(event);
    }
}

/// [`ClaudePlanningObserver`] with no lifecycle events forwarded anywhere.
/// Used by the [`chatoms_ports::planning::ClaudePlanningExecutor`] impl
/// below, which does not surface per-event progress (only the final
/// [`chatoms_ports::planning::PlanningExecutionStartOutcome`] matters to an
/// application-layer orchestrator).
struct NoopObserver;

impl ClaudePlanningObserver for NoopObserver {
    fn on_event(&mut self, _event: ProcessLifecycleEvent) {}
}

impl<C, S> chatoms_ports::planning::ClaudePlanningExecutor for ClaudePlanningAdapter<C, S>
where
    C: ProviderCapabilityPort,
    S: StreamingProcessRunner,
{
    /// Maps this adapter's three-way [`ClaudePlanningStartOutcome`] onto the
    /// port's two-way
    /// [`chatoms_ports::planning::PlanningExecutionStartOutcome`]:
    /// `StdinTooLarge` folds into `PreflightRejected` because both mean "no
    /// subprocess was started" and an application-layer orchestrator treats
    /// them identically (fail-closed to `RecoveryRequired`), mirroring
    /// [`crate::claude_implementation`]'s identical fold.
    fn start_planning(
        &mut self,
        worktree: &Path,
        brief: chatoms_ports::planning::PlanningExecutionBrief<'_>,
        cancellation: &dyn CancellationSignal,
    ) -> Result<chatoms_ports::planning::PlanningExecutionStartOutcome, PortFailure> {
        let mapped_brief = PlanningBrief {
            requirements: brief.requirements,
            completion_criteria: brief.completion_criteria,
            prohibited_scope: brief.prohibited_scope,
        };
        let mut observer = NoopObserver;
        match ClaudePlanningAdapter::start_planning(
            self,
            worktree,
            mapped_brief,
            cancellation,
            &mut observer,
        )? {
            ClaudePlanningStartOutcome::PreflightRejected
            | ClaudePlanningStartOutcome::StdinTooLarge => {
                Ok(chatoms_ports::planning::PlanningExecutionStartOutcome::PreflightRejected)
            }
            ClaudePlanningStartOutcome::Completed(result) => Ok(
                chatoms_ports::planning::PlanningExecutionStartOutcome::Completed(
                    chatoms_ports::planning::PlanningExecutionResult {
                        outcome: result.outcome,
                        exit_code: result.exit_code,
                        turn_count: result.turn_count,
                        plan_text: result.plan_text,
                    },
                ),
            ),
        }
    }
}

fn planning_arguments(worktree: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-p"),
        OsString::from("--permission-mode"),
        OsString::from("plan"),
        OsString::from("--tools"),
        OsString::from(ALLOWED_TOOLS),
        OsString::from("--output-format"),
        OsString::from("json"),
        OsString::from("--add-dir"),
        worktree.as_os_str().to_owned(),
        OsString::from("--max-turns"),
        OsString::from(MAX_TURNS),
        OsString::from("--strict-mcp-config"),
        OsString::from("--setting-sources"),
        OsString::from(SETTING_SOURCES),
        OsString::from("--disable-slash-commands"),
        OsString::from(FIXED_INSTRUCTION),
    ]
}

fn format_stdin(brief: &PlanningBrief<'_>) -> Vec<u8> {
    format!(
        "## Requirements\n{}\n\n## Completion Criteria\n{}\n\n## Prohibited Scope\n{}\n",
        brief.requirements, brief.completion_criteria, brief.prohibited_scope,
    )
    .into_bytes()
}

/// The subset of the CLI's `--output-format json` result envelope this
/// module trusts. Any field not listed here (`session_id`,
/// `total_cost_usd`, `duration_ms`, transcript/usage details, ...) is
/// simply never captured — `serde` ignores unknown fields by default, so
/// there is no field to accidentally carry them through.
#[derive(serde::Deserialize)]
struct ClaudeResultEnvelope {
    subtype: String,
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    num_turns: Option<u32>,
}

fn without_text(outcome: PlanningResultOutcome, exit_code: Option<i32>) -> ClaudePlanningResult {
    ClaudePlanningResult {
        outcome,
        exit_code,
        turn_count: None,
        plan_text: None,
    }
}

/// Reduces a completed streaming invocation to the Task-state-machine-ready
/// [`ClaudePlanningResult`]. `buffer` is the bounded stdout this module's
/// own [`ResultCapturingRelay`] accumulated; it is only ever inspected here
/// and only when `completion` reports a clean zero-exit `Completed`.
fn interpret_completion(
    completion: StreamingProcessCompletion,
    buffer: &[u8],
    redactor: &SecretRedactor,
) -> ClaudePlanningResult {
    match completion.outcome {
        StreamingOutcome::Cancelled => {
            without_text(PlanningResultOutcome::Cancelled, completion.exit_code)
        }
        StreamingOutcome::StdoutBoundExceeded => {
            without_text(PlanningResultOutcome::Failed, completion.exit_code)
        }
        // Planning's tool allowlist (`--tools Read,Glob,Grep`) makes an
        // external write effect structurally impossible: no tool capable
        // of one is ever available in the session. So "the process outcome
        // is unclear" can only be about local process/DB bookkeeping —
        // exactly `RecoveryRequired`'s definition in
        // docs/STATE_MACHINE.md — never `UnknownExternalEffect`'s ("an
        // external write's real-world effect is unknown"), which cannot
        // apply here by construction.
        StreamingOutcome::Uncertain => without_text(
            PlanningResultOutcome::RecoveryRequired,
            completion.exit_code,
        ),
        StreamingOutcome::Completed if completion.exit_code == Some(0) => {
            parse_success_result(completion.exit_code, buffer, redactor)
        }
        StreamingOutcome::Completed => {
            without_text(PlanningResultOutcome::Failed, completion.exit_code)
        }
    }
}

fn parse_success_result(
    exit_code: Option<i32>,
    buffer: &[u8],
    redactor: &SecretRedactor,
) -> ClaudePlanningResult {
    let Ok(envelope) = serde_json::from_slice::<ClaudeResultEnvelope>(buffer) else {
        return without_text(PlanningResultOutcome::RecoveryRequired, exit_code);
    };
    if envelope.subtype != "success" || envelope.is_error {
        return without_text(PlanningResultOutcome::RecoveryRequired, exit_code);
    }
    let Some(result_text) = envelope.result else {
        return without_text(PlanningResultOutcome::RecoveryRequired, exit_code);
    };
    let report = redactor.redact_text(&result_text);
    if report.failed_closed {
        return without_text(PlanningResultOutcome::RecoveryRequired, exit_code);
    }
    ClaudePlanningResult {
        outcome: PlanningResultOutcome::Completed,
        exit_code,
        turn_count: envelope.num_turns,
        plan_text: Some(report.text.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chatoms_ports::{
        error::{CategorizedFailure, FailureCategory},
        provider::ProviderCapabilities,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Clone)]
    struct FakeCapabilityPort {
        claude: ProviderCapabilityStatus,
        calls: Arc<AtomicUsize>,
        fail: bool,
    }

    impl FakeCapabilityPort {
        fn supported() -> Self {
            Self {
                claude: ProviderCapabilityStatus::Supported,
                calls: Arc::new(AtomicUsize::new(0)),
                fail: false,
            }
        }

        fn unsupported() -> Self {
            Self {
                claude: ProviderCapabilityStatus::Unsupported,
                calls: Arc::new(AtomicUsize::new(0)),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                claude: ProviderCapabilityStatus::Unsupported,
                calls: Arc::new(AtomicUsize::new(0)),
                fail: true,
            }
        }
    }

    impl ProviderCapabilityPort for FakeCapabilityPort {
        fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(PortFailure::new(FailureCategory::Unsupported));
            }
            Ok(ProviderCapabilities {
                claude: self.claude,
                codex: ProviderCapabilityStatus::Unsupported,
            })
        }
    }

    type ObservedRun = (ProcessSpec, Option<Vec<u8>>, usize);

    #[derive(Clone, Default)]
    struct FakeStreamingRunner {
        observed: Arc<Mutex<Vec<ObservedRun>>>,
        scripted: Option<StreamingProcessCompletion>,
        emit_stdout: Option<Vec<u8>>,
        emit_events: Vec<ProcessLifecycleEvent>,
    }

    impl StreamingProcessRunner for FakeStreamingRunner {
        fn run_streaming(
            &mut self,
            spec: &ProcessSpec,
            stdin: Option<&[u8]>,
            max_stdout_bytes: usize,
            _cancellation: &dyn CancellationSignal,
            observer: &mut dyn StreamingProcessObserver,
        ) -> Result<StreamingProcessCompletion, PortFailure> {
            self.observed.lock().expect("observed lock").push((
                spec.clone(),
                stdin.map(<[u8]>::to_vec),
                max_stdout_bytes,
            ));
            if let Some(chunk) = &self.emit_stdout {
                observer.on_stdout_chunk(chunk);
            }
            for event in &self.emit_events {
                observer.on_event(*event);
            }
            self.scripted
                .ok_or_else(|| PortFailure::new(FailureCategory::Unsupported))
        }
    }

    #[derive(Default)]
    struct SpyObserver {
        events: Vec<ProcessLifecycleEvent>,
    }

    impl ClaudePlanningObserver for SpyObserver {
        fn on_event(&mut self, event: ProcessLifecycleEvent) {
            self.events.push(event);
        }
    }

    fn never_cancelled() -> chatoms_ports::process::AtomicCancellationSignal {
        chatoms_ports::process::AtomicCancellationSignal::new()
    }

    fn brief() -> PlanningBrief<'static> {
        PlanningBrief {
            requirements: "Add CSV export",
            completion_criteria: "Export button downloads a CSV",
            prohibited_scope: "Do not touch the import pipeline",
        }
    }

    fn completed(exit_code: i32) -> StreamingProcessCompletion {
        StreamingProcessCompletion {
            outcome: chatoms_ports::process::StreamingOutcome::Completed,
            exit_code: Some(exit_code),
        }
    }

    fn streaming_completion(
        outcome: StreamingOutcome,
        exit_code: Option<i32>,
    ) -> StreamingProcessCompletion {
        StreamingProcessCompletion { outcome, exit_code }
    }

    fn redactor() -> SecretRedactor {
        SecretRedactor::new().expect("redactor rules compile")
    }

    fn success_json(result: &str, num_turns: u32) -> Vec<u8> {
        format!(
            r#"{{"subtype":"success","is_error":false,"result":{},"num_turns":{num_turns},"session_id":"leak-if-parsed","total_cost_usd":0.02}}"#,
            serde_json::to_string(result).expect("json-encode result text"),
        )
        .into_bytes()
    }

    fn make_adapter(
        capability: FakeCapabilityPort,
        streaming: FakeStreamingRunner,
    ) -> ClaudePlanningAdapter<FakeCapabilityPort, FakeStreamingRunner> {
        ClaudePlanningAdapter::new(
            capability,
            streaming,
            PathBuf::from("C:/trusted/claude.exe"),
            PathBuf::from("C:/preflight/provider-preflight"),
            redactor(),
        )
    }

    fn run_once(
        adapter: &mut ClaudePlanningAdapter<FakeCapabilityPort, FakeStreamingRunner>,
    ) -> ClaudePlanningStartOutcome {
        let cancellation = never_cancelled();
        let mut observer = SpyObserver::default();
        adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                brief(),
                &cancellation,
                &mut observer,
            )
            .expect("start planning")
    }

    fn completed_result(outcome: &ClaudePlanningStartOutcome) -> &ClaudePlanningResult {
        match outcome {
            ClaudePlanningStartOutcome::Completed(result) => result,
            ClaudePlanningStartOutcome::PreflightRejected => {
                panic!("expected a completed run, got PreflightRejected")
            }
            ClaudePlanningStartOutcome::StdinTooLarge => {
                panic!("expected a completed run, got StdinTooLarge")
            }
        }
    }

    #[test]
    fn spawns_with_the_approved_read_only_argv_cwd_and_stdin() {
        let capability = FakeCapabilityPort::supported();
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json("do the thing", 5)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(capability, streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::Completed);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.turn_count, Some(5));
        assert_eq!(result.plan_text.as_deref(), Some("do the thing"));

        let runs = observed.lock().expect("observed lock");
        assert_eq!(runs.len(), 1);
        let (spec, stdin, max_bytes) = &runs[0];
        assert_eq!(spec.executable, Path::new("C:/trusted/claude.exe"));
        assert_eq!(
            spec.working_directory,
            Path::new("C:/preflight/provider-preflight"),
            "CWD must be the trusted preflight directory, never the worktree"
        );
        assert_eq!(
            spec.arguments,
            vec![
                OsString::from("-p"),
                OsString::from("--permission-mode"),
                OsString::from("plan"),
                OsString::from("--tools"),
                OsString::from("Read,Glob,Grep"),
                OsString::from("--output-format"),
                OsString::from("json"),
                OsString::from("--add-dir"),
                OsString::from("C:/managed/task-worktree"),
                OsString::from("--max-turns"),
                OsString::from("12"),
                OsString::from("--strict-mcp-config"),
                OsString::from("--setting-sources"),
                OsString::from("project,local"),
                OsString::from("--disable-slash-commands"),
                OsString::from(FIXED_INSTRUCTION),
            ]
        );
        let stdin = stdin.as_ref().expect("stdin must be provided");
        let stdin_text = String::from_utf8(stdin.clone()).expect("utf8 stdin");
        assert!(stdin_text.contains("## Requirements\nAdd CSV export"));
        assert!(stdin_text.contains("## Completion Criteria\nExport button downloads a CSV"));
        assert!(stdin_text.contains("## Prohibited Scope\nDo not touch the import pipeline"));
        assert_eq!(*max_bytes, MAX_STDOUT_BYTES);
    }

    #[test]
    fn only_the_final_structured_result_field_is_kept_never_session_metadata() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json("the plan text", 3)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.plan_text.as_deref(), Some("the plan text"));
        let rendered = format!("{result:?}");
        assert!(
            !rendered.contains("leak-if-parsed"),
            "session_id must never be captured, even though the CLI includes it"
        );
        assert!(
            !rendered.contains("total_cost_usd") && !rendered.contains("0.02"),
            "cost metadata must never be captured"
        );
    }

    #[test]
    fn malformed_or_non_success_json_never_becomes_a_stored_plan() {
        let cases: [(&str, &[u8]); 4] = [
            (
                "not json at all",
                b"tool output the observer must never see",
            ),
            (
                "error subtype despite zero exit",
                br#"{"subtype":"error_max_turns","is_error":true,"result":"partial plan leaked?"}"#,
            ),
            (
                "success subtype but missing result field",
                br#"{"subtype":"success","is_error":false,"num_turns":2}"#,
            ),
            ("empty stdout", b""),
        ];
        for (label, body) in cases {
            let streaming = FakeStreamingRunner {
                scripted: Some(completed(0)),
                emit_stdout: Some(body.to_vec()),
                ..FakeStreamingRunner::default()
            };
            let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);
            let outcome = run_once(&mut adapter);
            let result = completed_result(&outcome);
            assert_eq!(
                result.outcome,
                PlanningResultOutcome::RecoveryRequired,
                "case: {label}"
            );
            assert_eq!(result.plan_text, None, "case: {label}");
            assert_eq!(result.turn_count, None, "case: {label}");
        }
    }

    #[test]
    fn result_text_is_masked_before_it_leaves_the_adapter() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json(
                "Plan: read config.json which has api_key: \"sk-abcdefghijklmnopqrst\" inside it",
                4,
            )),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::Completed);
        let text = result.plan_text.as_deref().expect("masked plan text");
        assert!(!text.contains("sk-abcdefghijklmnopqrst"));
        assert!(text.contains("[REDACTED"));
    }

    #[test]
    fn result_text_that_fails_closed_when_masking_cannot_certify_it_safe_is_recovery_required() {
        // Percent-encoded so no direct rule matches the raw text (no
        // replacement happens), but decoding once reveals an
        // `api_key: ...` pattern the redactor's own sensitivity check
        // recognizes. `SecretRedactor::redact_text` treats "sensitive only
        // once decoded, with zero direct replacements" as unsafe to certify
        // and fails closed rather than emitting a result it cannot vouch
        // for. This Unit's contract treats that as a mask/cap failure:
        // `RecoveryRequired`, never a stored "plan".
        let poisoned = "See api%5Fkey%3A%20supersecretvalue123456 in the config.";
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json(poisoned, 1)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::RecoveryRequired);
        assert_eq!(result.plan_text, None);
    }

    #[test]
    fn oversized_result_text_is_capped_not_stored_unbounded() {
        let huge = "a".repeat(crate::redaction::MAX_REDACTION_INPUT_BYTES + 5_000);
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json(&huge, 1)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::Completed);
        let text = result.plan_text.as_ref().expect("capped plan text");
        assert!(
            text.len() < huge.len(),
            "capped text must be smaller than the raw oversized result"
        );
        assert!(text.contains(crate::redaction::TRUNCATED_MARKER));
    }

    #[test]
    fn nonzero_exit_on_a_completed_run_maps_to_failed() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(1)),
            emit_stdout: Some(success_json("should never be read", 1)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::Failed);
        assert_eq!(result.exit_code, Some(1));
        assert_eq!(result.plan_text, None);
    }

    #[test]
    fn stdout_bound_exceeded_maps_to_failed() {
        let streaming = FakeStreamingRunner {
            scripted: Some(streaming_completion(
                StreamingOutcome::StdoutBoundExceeded,
                None,
            )),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::Failed);
        assert_eq!(result.plan_text, None);
    }

    #[test]
    fn confirmed_cancellation_maps_to_cancelled() {
        let streaming = FakeStreamingRunner {
            scripted: Some(streaming_completion(StreamingOutcome::Cancelled, None)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::Cancelled);
        assert_eq!(result.plan_text, None);
    }

    #[test]
    fn uncertain_outcome_maps_to_recovery_required_never_unknown_external_effect() {
        // Rationale fixed by this test: Planning's tool allowlist
        // (`--tools Read,Glob,Grep`) makes an external write structurally
        // impossible, so an uncertain process outcome can only be about
        // local bookkeeping (`RecoveryRequired`), never an external effect
        // whose real-world result is unknown (`UnknownExternalEffect`).
        let streaming = FakeStreamingRunner {
            scripted: Some(streaming_completion(StreamingOutcome::Uncertain, None)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        let outcome = run_once(&mut adapter);

        let result = completed_result(&outcome);
        assert_eq!(result.outcome, PlanningResultOutcome::RecoveryRequired);
        assert_eq!(result.plan_text, None);
    }

    #[test]
    fn worktree_is_never_used_as_the_working_directory() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json("plan", 1)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        adapter
            .start_planning(
                Path::new("C:/some/other/worktree"),
                brief(),
                &never_cancelled(),
                &mut SpyObserver::default(),
            )
            .expect("start planning");

        let runs = observed.lock().expect("observed lock");
        assert_ne!(
            runs[0].0.working_directory,
            Path::new("C:/some/other/worktree")
        );
        assert!(
            runs[0]
                .0
                .arguments
                .windows(2)
                .any(|pair| pair[0] == "--add-dir" && pair[1] == "C:/some/other/worktree"),
            "worktree must be passed only as a --add-dir read-access argument"
        );
    }

    #[test]
    fn fresh_preflight_rejection_prevents_any_spawn() {
        let capability = FakeCapabilityPort::unsupported();
        let calls = capability.calls.clone();
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(capability, streaming);
        let cancellation = never_cancelled();
        let mut observer = SpyObserver::default();

        let outcome = adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                brief(),
                &cancellation,
                &mut observer,
            )
            .expect("typed fail-closed result, not an error");

        assert_eq!(outcome, ClaudePlanningStartOutcome::PreflightRejected);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "fresh check must run");
        assert!(
            observed.lock().expect("observed lock").is_empty(),
            "no subprocess may be started when fresh validation fails"
        );
        assert!(observer.events.is_empty());
    }

    #[test]
    fn capability_port_failure_prevents_any_spawn() {
        let mut adapter = make_adapter(
            FakeCapabilityPort::failing(),
            FakeStreamingRunner {
                scripted: Some(completed(0)),
                ..FakeStreamingRunner::default()
            },
        );
        let observed_runner = FakeStreamingRunner::default();
        let observed = observed_runner.observed.clone();
        let cancellation = never_cancelled();
        let mut observer = SpyObserver::default();

        let error = adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                brief(),
                &cancellation,
                &mut observer,
            )
            .expect_err("capability port failure must surface, not be swallowed");

        assert_eq!(error.category(), FailureCategory::Unsupported);
        assert!(observed.lock().expect("observed lock").is_empty());
    }

    #[test]
    fn every_call_re_runs_the_fresh_capability_check_even_when_supported_repeatedly() {
        let capability = FakeCapabilityPort::supported();
        let calls = capability.calls.clone();
        let mut adapter = make_adapter(
            capability,
            FakeStreamingRunner {
                scripted: Some(completed(0)),
                emit_stdout: Some(success_json("plan", 1)),
                ..FakeStreamingRunner::default()
            },
        );

        for _ in 0..3 {
            run_once(&mut adapter);
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "an earlier Supported result must never be cached across calls"
        );
    }

    #[test]
    fn stdout_bytes_never_reach_the_caller_observer_only_content_free_events_do() {
        const CANARY: &[u8] = b"tool output the observer must never see";
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(CANARY.to_vec()),
            emit_events: vec![
                ProcessLifecycleEvent::Started,
                ProcessLifecycleEvent::StdoutChunkReceived {
                    byte_len: CANARY.len(),
                },
                ProcessLifecycleEvent::Exited { exit_code: Some(0) },
            ],
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);
        let cancellation = never_cancelled();
        let mut observer = SpyObserver::default();

        adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                brief(),
                &cancellation,
                &mut observer,
            )
            .expect("start planning");

        assert_eq!(
            observer.events,
            vec![
                ProcessLifecycleEvent::Started,
                ProcessLifecycleEvent::StdoutChunkReceived {
                    byte_len: CANARY.len()
                },
                ProcessLifecycleEvent::Exited { exit_code: Some(0) },
            ]
        );
        // `ClaudePlanningObserver` has no raw-byte method at all, so this is
        // also enforced at compile time: the assertion above only proves
        // the wiring in this adapter actually uses that guarantee.
    }

    #[test]
    fn bounded_stdout_budget_is_fixed_by_the_adapter_not_the_caller() {
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json("plan", 1)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);

        run_once(&mut adapter);

        assert_eq!(
            observed.lock().expect("observed lock")[0].2,
            MAX_STDOUT_BYTES
        );
    }

    #[test]
    fn oversized_stdin_is_rejected_before_any_spawn() {
        let oversized_requirements = "a".repeat(MAX_STDIN_BYTES);
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);
        let oversized_brief = PlanningBrief {
            requirements: &oversized_requirements,
            completion_criteria: "c",
            prohibited_scope: "p",
        };

        let outcome = adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                oversized_brief,
                &never_cancelled(),
                &mut SpyObserver::default(),
            )
            .expect("typed fail-closed result, not an error");

        assert_eq!(outcome, ClaudePlanningStartOutcome::StdinTooLarge);
        assert!(
            observed.lock().expect("observed lock").is_empty(),
            "no subprocess may be started when stdin exceeds the cap"
        );
    }

    #[test]
    fn stdin_at_or_under_the_cap_is_accepted() {
        let requirements_within_cap = "a".repeat(MAX_STDIN_BYTES / 4);
        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json("plan", 1)),
            ..FakeStreamingRunner::default()
        };
        let observed = streaming.observed.clone();
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);
        let within_cap_brief = PlanningBrief {
            requirements: &requirements_within_cap,
            completion_criteria: "c",
            prohibited_scope: "p",
        };

        let outcome = adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                within_cap_brief,
                &never_cancelled(),
                &mut SpyObserver::default(),
            )
            .expect("start planning");

        assert!(matches!(outcome, ClaudePlanningStartOutcome::Completed(_)));
        assert_eq!(observed.lock().expect("observed lock").len(), 1);
    }

    #[test]
    fn run_streaming_failure_after_passed_preflight_still_surfaces_as_an_error() {
        let mut adapter = make_adapter(
            FakeCapabilityPort::supported(),
            FakeStreamingRunner {
                scripted: None,
                ..FakeStreamingRunner::default()
            },
        );
        let cancellation = never_cancelled();
        let mut observer = SpyObserver::default();

        adapter
            .start_planning(
                Path::new("C:/managed/task-worktree"),
                brief(),
                &cancellation,
                &mut observer,
            )
            .expect_err("a genuine spawn failure must not be silently swallowed");
    }

    #[test]
    fn claude_planning_executor_port_impl_delegates_to_the_inherent_method() {
        use chatoms_ports::planning::{
            ClaudePlanningExecutor, PlanningExecutionBrief, PlanningExecutionStartOutcome,
        };

        let streaming = FakeStreamingRunner {
            scripted: Some(completed(0)),
            emit_stdout: Some(success_json("the plan text", 3)),
            ..FakeStreamingRunner::default()
        };
        let mut adapter = make_adapter(FakeCapabilityPort::supported(), streaming);
        let cancellation = never_cancelled();

        let outcome = ClaudePlanningExecutor::start_planning(
            &mut adapter,
            Path::new("C:/managed/task-worktree"),
            PlanningExecutionBrief {
                requirements: "Add CSV export",
                completion_criteria: "Export button downloads a CSV",
                prohibited_scope: "Do not touch the import pipeline",
            },
            &cancellation,
        )
        .expect("port-level start_planning");

        let PlanningExecutionStartOutcome::Completed(result) = outcome else {
            panic!("expected a completed run");
        };
        assert_eq!(result.outcome, PlanningResultOutcome::Completed);
        assert_eq!(result.plan_text.as_deref(), Some("the plan text"));
        assert_eq!(result.turn_count, Some(3));
    }

    #[test]
    fn claude_planning_executor_port_impl_reports_preflight_rejection() {
        use chatoms_ports::planning::{
            ClaudePlanningExecutor, PlanningExecutionBrief, PlanningExecutionStartOutcome,
        };

        let mut adapter = make_adapter(
            FakeCapabilityPort::unsupported(),
            FakeStreamingRunner::default(),
        );
        let cancellation = never_cancelled();

        let outcome = ClaudePlanningExecutor::start_planning(
            &mut adapter,
            Path::new("C:/managed/task-worktree"),
            PlanningExecutionBrief {
                requirements: "r",
                completion_criteria: "c",
                prohibited_scope: "p",
            },
            &cancellation,
        )
        .expect("typed fail-closed result, not an error");

        assert_eq!(outcome, PlanningExecutionStartOutcome::PreflightRejected);
    }

    #[test]
    fn claude_planning_executor_port_impl_folds_stdin_too_large_into_preflight_rejected() {
        use chatoms_ports::planning::{
            ClaudePlanningExecutor, PlanningExecutionBrief, PlanningExecutionStartOutcome,
        };

        let oversized_requirements = "a".repeat(MAX_STDIN_BYTES);
        let mut adapter = make_adapter(
            FakeCapabilityPort::supported(),
            FakeStreamingRunner::default(),
        );
        let cancellation = never_cancelled();

        let outcome = ClaudePlanningExecutor::start_planning(
            &mut adapter,
            Path::new("C:/managed/task-worktree"),
            PlanningExecutionBrief {
                requirements: &oversized_requirements,
                completion_criteria: "c",
                prohibited_scope: "p",
            },
            &cancellation,
        )
        .expect("typed fail-closed result, not an error");

        assert_eq!(
            outcome,
            PlanningExecutionStartOutcome::PreflightRejected,
            "StdinTooLarge must fold into PreflightRejected at the port boundary, matching Implementation"
        );
    }
}
