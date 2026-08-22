use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use chatoms_platform::git_runtime::TrustedGitRuntime;
use chatoms_platform::{ensure_supported_directory, supported_directory_identity};

use chatoms_ports::{
    diff::{
        CommitCandidate, CommitCandidateOutcome, CommitCandidatePort, DiffContentHash,
        WorktreeDiff, WorktreeDiffOutcome, WorktreeDiffPort,
    },
    error::{FailureCategory, PortFailure},
    filesystem::DirectoryIdentity,
    git::{
        GitService, ProjectInspection, RepositoryKind, RepositorySafetyToken, RepositoryStatus,
        WorktreeCreationOutcome,
    },
};

#[cfg(not(windows))]
#[derive(Clone, Debug)]
struct TrustedGitRuntime;

#[cfg(not(windows))]
impl TrustedGitRuntime {
    fn discover() -> Result<Self, ()> {
        Err(())
    }

    fn executable(&self) -> &Path {
        Path::new("")
    }

    fn cmd(&self) -> &Path {
        Path::new("")
    }

    fn bin(&self) -> &Path {
        Path::new("")
    }

    fn exec_path(&self) -> &Path {
        Path::new("")
    }

    fn system_directory(&self) -> &Path {
        Path::new("")
    }

    fn system_root(&self) -> &Path {
        Path::new("")
    }

    fn validate(&self) -> Result<(), ()> {
        Err(())
    }

    fn user_global_config_path() -> Result<PathBuf, ()> {
        Err(())
    }
}

const INITIAL_SNAPSHOT_MESSAGE: &str = "chore: create initial project snapshot";

/// Byte bound on a single worktree diff read. Small and explicit: this diff
/// is only ever meant to become part of a future Claude Review stdin
/// payload alongside a fixed template and the stored plan text, not a
/// general-purpose diff viewer.
const DIFF_MAX_BYTES: usize = 512 * 1024;

/// Wall-clock bound on a single worktree diff read. `git diff` against a
/// worktree's own `HEAD` is expected to be fast; this exists only to force
/// termination if the process hangs (e.g. a stuck filesystem).
const DIFF_TIMEOUT: Duration = Duration::from_secs(20);

/// Poll interval while waiting for the diff process to exit or the deadline
/// to pass.
const DIFF_POLL_INTERVAL: Duration = Duration::from_millis(20);
const GIT_WRITE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitWriteCommand {
    Stage,
    Commit,
    Merge,
    MergeAbort,
}

pub trait GitWriteCommandObserver: Send + Sync {
    fn before_command(&self, command: GitWriteCommand);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitWriteCommandOutcome {
    Succeeded,
    Failed,
    TimedOut,
    Uncertain,
}

#[derive(Clone, Debug)]
struct GitControlPaths {
    home: PathBuf,
    hooks: PathBuf,
    template: PathBuf,
    global_config: PathBuf,
    global_attributes: PathBuf,
}

#[derive(Clone)]
pub struct GitCliAdapter {
    runtime: TrustedGitRuntime,
    control: GitControlPaths,
    control_identity: Option<Vec<(PathBuf, String)>>,
    control_directory_identity: Option<Vec<DirectoryIdentity>>,
    write_observer: Option<Arc<dyn GitWriteCommandObserver>>,
}

impl GitCliAdapter {
    pub fn from_environment() -> Result<Self, PortFailure> {
        let base = env::var_os("LOCALAPPDATA")
            .ok_or_else(|| PortFailure::new(FailureCategory::Unsupported))?;
        Self::with_control_root(
            PathBuf::from(base).join("ChatOMS").join("git-control"),
            false,
        )
    }

    pub fn new(control_root: PathBuf) -> Result<Self, PortFailure> {
        Self::with_control_root(control_root, true)
    }

    fn with_control_root(control_root: PathBuf, prepare_now: bool) -> Result<Self, PortFailure> {
        let runtime = discover_trusted_git_runtime()?;
        let control = git_control_paths(&control_root);
        let mut adapter = Self {
            runtime,
            control,
            control_identity: None,
            control_directory_identity: None,
            write_observer: None,
        };
        if prepare_now {
            adapter.ensure_control_paths()?;
        }
        Ok(adapter)
    }

    fn output(
        &mut self,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Output, PortFailure> {
        self.output_with_input(arguments, None)
    }

    fn output_with_input(
        &mut self,
        arguments: impl IntoIterator<Item = OsString>,
        input: Option<&[u8]>,
    ) -> Result<Output, PortFailure> {
        self.ensure_control_paths()?;
        self.validate_runtime()?;
        self.validate_control_paths()?;
        let mut command = Command::new(self.runtime.executable());
        command
            .env_clear()
            .current_dir(&self.control.home)
            .args(self.common_arguments())
            .args(arguments)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_controlled_environment(&mut command, &self.runtime, &self.control)?;
        let mut child = command.spawn().map_err(map_spawn_error)?;
        let Some(input) = input else {
            return child.wait_with_output().map_err(map_io_error);
        };
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PortFailure::new(FailureCategory::Internal))?;
        // The writer runs on its own thread so stdout/stderr draining below
        // starts immediately instead of waiting for the full stdin write;
        // otherwise a child that emits output before consuming all of stdin
        // (e.g. `git check-attr --stdin`) can deadlock both pipes.
        thread::scope(|scope| {
            let writer = scope.spawn(move || stdin.write_all(input));
            let output = child.wait_with_output().map_err(map_io_error);
            match writer.join() {
                Ok(Ok(())) => output,
                Ok(Err(write_error)) => Err(map_io_error(write_error)),
                Err(_) => Err(PortFailure::new(FailureCategory::Internal)),
            }
        })
    }

    fn author_output(
        &mut self,
        root: &Path,
        key: &str,
        local: bool,
    ) -> Result<Output, PortFailure> {
        self.ensure_control_paths()?;
        self.validate_runtime()?;
        self.validate_control_paths()?;
        let mut command = Command::new(self.runtime.executable());
        command
            .env_clear()
            .current_dir(&self.control.home)
            .args(self.common_arguments())
            .args([OsString::from("-C"), root.as_os_str().to_owned()]);
        if local {
            command.args(["config", "--local", "--get", key]);
        } else {
            command.args(["config", "--global", "--get", key]);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_controlled_environment(&mut command, &self.runtime, &self.control)?;
        if !local {
            command.env(
                "GIT_CONFIG_GLOBAL",
                TrustedGitRuntime::user_global_config_path()
                    .map_err(|_| PortFailure::new(FailureCategory::Unsupported))?,
            );
        }
        command.output().map_err(map_spawn_error)
    }

    fn common_arguments(&self) -> Vec<OsString> {
        let configs = [
            ("core.hooksPath", self.control.hooks.as_os_str()),
            (
                "core.attributesFile",
                self.control.global_attributes.as_os_str(),
            ),
            (
                "core.excludesFile",
                self.control.global_attributes.as_os_str(),
            ),
        ];
        let mut arguments = vec![
            OsString::from("--no-pager"),
            OsString::from("--no-lazy-fetch"),
        ];
        for (key, value) in configs {
            arguments.push(OsString::from("-c"));
            let mut config = OsString::from(key);
            config.push("=");
            config.push(value);
            arguments.push(config);
        }
        for config in [
            "core.fsmonitor=false",
            "commit.gpgSign=false",
            "credential.helper=",
            "protocol.allow=never",
            "protocol.file.allow=never",
            "submodule.recurse=false",
        ] {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(config));
        }
        arguments
    }

    fn at(
        &mut self,
        root: &Path,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Output, PortFailure> {
        let mut args = vec![OsString::from("-C"), root.as_os_str().to_owned()];
        args.extend(arguments);
        self.output(args)
    }

    fn at_str<const N: usize>(
        &mut self,
        root: &Path,
        arguments: [&str; N],
    ) -> Result<Output, PortFailure> {
        self.at(root, arguments.into_iter().map(OsString::from))
    }

    fn at_with_input(
        &mut self,
        root: &Path,
        arguments: impl IntoIterator<Item = OsString>,
        input: &[u8],
    ) -> Result<Output, PortFailure> {
        let mut args = vec![OsString::from("-C"), root.as_os_str().to_owned()];
        args.extend(arguments);
        self.output_with_input(args, Some(input))
    }

    pub(crate) fn run_command<const N: usize>(
        &mut self,
        root: &Path,
        arguments: [&str; N],
    ) -> Result<Output, PortFailure> {
        self.at_str(root, arguments)
    }

    pub(crate) fn capture_read_only(
        &mut self,
        root: &Path,
        arguments: &[&str],
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<BoundedCaptureOutcome, PortFailure> {
        if !root.is_absolute() {
            return Err(PortFailure::new(FailureCategory::InvalidInput));
        }
        self.ensure_control_paths()?;
        self.validate_runtime()?;
        self.validate_control_paths()?;
        let mut command = Command::new(self.runtime.executable());
        command
            .env_clear()
            .current_dir(&self.control.home)
            .args(self.common_arguments())
            .arg("-C")
            .arg(root)
            .args(arguments);
        configure_controlled_environment(&mut command, &self.runtime, &self.control)?;
        capture_bounded_stdout(command, max_bytes, timeout)
    }

    pub fn set_write_command_observer(
        &mut self,
        observer: Option<Arc<dyn GitWriteCommandObserver>>,
    ) {
        self.write_observer = observer;
    }

    pub(crate) fn run_write_command<const N: usize>(
        &mut self,
        root: &Path,
        command_kind: GitWriteCommand,
        arguments: [&str; N],
    ) -> GitWriteCommandOutcome {
        self.run_write_command_with_env(root, command_kind, arguments, &[])
    }

    /// Identical to [`Self::run_write_command`], but adds `extra_env`
    /// key/value pairs to the child process's environment on top of the
    /// shared controlled environment — never logged or persisted by this
    /// method. Used only where a write must not depend on ambient Git
    /// config for author identity (e.g. `merge --continue`).
    pub(crate) fn run_write_command_with_env<const N: usize>(
        &mut self,
        root: &Path,
        command_kind: GitWriteCommand,
        arguments: [&str; N],
        extra_env: &[(&str, &OsStr)],
    ) -> GitWriteCommandOutcome {
        let mut args = vec![OsString::from("-C"), root.as_os_str().to_owned()];
        args.extend(arguments.into_iter().map(OsString::from));
        let deadline = Instant::now() + GIT_WRITE_TIMEOUT;
        let result = self.bounded_write_output(args, command_kind, deadline, extra_env);
        match result {
            Ok(BoundedCaptureOutcome::Success(_)) => GitWriteCommandOutcome::Succeeded,
            Ok(BoundedCaptureOutcome::ExitFailure) => GitWriteCommandOutcome::Failed,
            Ok(BoundedCaptureOutcome::TimedOut) => GitWriteCommandOutcome::TimedOut,
            Ok(BoundedCaptureOutcome::TooLarge | BoundedCaptureOutcome::Uncertain) | Err(_) => {
                GitWriteCommandOutcome::Uncertain
            }
        }
    }

    fn bounded_write_output(
        &mut self,
        arguments: Vec<OsString>,
        command_kind: GitWriteCommand,
        deadline: Instant,
        extra_env: &[(&str, &OsStr)],
    ) -> Result<BoundedCaptureOutcome, PortFailure> {
        self.ensure_control_paths()?;
        self.validate_runtime()?;
        self.validate_control_paths()?;
        if let Some(observer) = &self.write_observer {
            observer.before_command(command_kind);
        }
        let Some(timeout) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(BoundedCaptureOutcome::TimedOut);
        };
        let mut command = Command::new(self.runtime.executable());
        command
            .env_clear()
            .current_dir(&self.control.home)
            .args(self.common_arguments())
            .args(arguments);
        configure_controlled_environment(&mut command, &self.runtime, &self.control)?;
        for (key, value) in extra_env {
            command.env(key, value);
        }
        capture_bounded_stdout(command, DIFF_MAX_BYTES, timeout)
    }

    /// Resolves the commit author/committer identity (`user.name`,
    /// `user.email`) the same way Git itself would for `root` — local
    /// config first, falling back to global — without ever logging or
    /// persisting the resolved values. Returns `None` if either value is
    /// missing or empty at both scopes, mirroring [`Self::has_commit_author`]'s
    /// precedence exactly.
    pub(crate) fn commit_author_identity(
        &mut self,
        root: &Path,
    ) -> Result<Option<(String, String)>, PortFailure> {
        let name = self.resolved_config_value(root, "user.name")?;
        let email = self.resolved_config_value(root, "user.email")?;
        Ok(match (name, email) {
            (Some(name), Some(email)) => Some((name, email)),
            _ => None,
        })
    }

    fn resolved_config_value(
        &mut self,
        root: &Path,
        key: &str,
    ) -> Result<Option<String>, PortFailure> {
        let local = self.author_output(root, key, true)?;
        if local.status.success() {
            let value = trimmed_utf8(&local.stdout)?;
            if !value.is_empty() {
                return Ok(Some(value.to_owned()));
            }
        }
        let global = self.author_output(root, key, false)?;
        if global.status.success() {
            let value = trimmed_utf8(&global.stdout)?;
            if !value.is_empty() {
                return Ok(Some(value.to_owned()));
            }
        }
        Ok(None)
    }

    pub(crate) fn output_text(output: &Output) -> Result<&str, PortFailure> {
        trimmed_utf8(&output.stdout)
    }

    pub(crate) fn validate_write_configuration(
        &mut self,
        root: &Path,
        worktree: &Path,
        base_commit: &str,
    ) -> Result<(), PortFailure> {
        self.validate_repository_source(root, base_commit)?;
        reject_active_info_attributes(root)?;
        self.check_worktree_filter_attributes(worktree)
    }

    fn validate_control_paths(&self) -> Result<(), PortFailure> {
        let control_identity = self
            .control_identity
            .as_ref()
            .ok_or_else(|| PortFailure::new(FailureCategory::StorageInsecure))?;
        let control_directory_identity = self
            .control_directory_identity
            .as_ref()
            .ok_or_else(|| PortFailure::new(FailureCategory::StorageInsecure))?;
        for directory in [
            &self.control.home,
            &self.control.hooks,
            &self.control.template,
        ] {
            let metadata = fs::symlink_metadata(directory).map_err(map_io_error)?;
            if !metadata.is_dir() || is_reparse_point(&metadata) {
                return Err(PortFailure::new(FailureCategory::Conflict));
            }
        }
        if capture_control_directory_identity(&self.control)? != *control_directory_identity {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        for file in [&self.control.global_config, &self.control.global_attributes] {
            let metadata = fs::symlink_metadata(file).map_err(map_io_error)?;
            if !metadata.is_file() || is_reparse_point(&metadata) || metadata.len() != 0 {
                return Err(PortFailure::new(FailureCategory::Conflict));
            }
        }
        if fs::read_dir(&self.control.hooks)
            .map_err(map_io_error)?
            .next()
            .is_some()
            || fs::read_dir(&self.control.template)
                .map_err(map_io_error)?
                .next()
                .is_some()
        {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        if capture_control_identity(&self.control)? != *control_identity {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        Ok(())
    }

    fn ensure_control_paths(&mut self) -> Result<(), PortFailure> {
        if self.control_identity.is_none() {
            prepare_control_paths(&self.control)?;
            self.control_identity = Some(capture_control_identity(&self.control)?);
            self.control_directory_identity =
                Some(capture_control_directory_identity(&self.control)?);
        }
        self.validate_control_paths()
    }

    fn validate_runtime(&self) -> Result<(), PortFailure> {
        validate_trusted_runtime(&self.runtime)
    }

    fn reject_local_dangerous_config(&mut self, root: &Path) -> Result<(), PortFailure> {
        let output = self.at_str(
            root,
            ["config", "--local", "--name-only", "--get-regexp", ".*"],
        )?;
        ensure_success(&output)?;
        for name in output.stdout.split(|byte| *byte == b'\n') {
            let name = std::str::from_utf8(name)
                .map_err(|_| PortFailure::new(FailureCategory::InvalidInput))?
                .trim()
                .to_ascii_lowercase();
            if name.starts_with("filter.")
                || name == "include.path"
                || name.starts_with("includeif.")
            {
                return Err(PortFailure::new(FailureCategory::Unsupported));
            }
        }
        Ok(())
    }

    fn check_filter_attributes(
        &mut self,
        root: &Path,
        source: Option<&str>,
        paths: &[u8],
    ) -> Result<(), PortFailure> {
        let mut args = vec![OsString::from("check-attr")];
        if let Some(source) = source {
            args.push(OsString::from(format!("--source={source}")));
        } else {
            args.push(OsString::from("--cached"));
        }
        args.extend([
            OsString::from("-z"),
            OsString::from("--stdin"),
            OsString::from("filter"),
        ]);
        let output = self.at_with_input(root, args, paths)?;
        ensure_success(&output)?;
        ensure_no_active_filter_output(&output.stdout)
    }

    fn check_worktree_filter_attributes(&mut self, root: &Path) -> Result<(), PortFailure> {
        let paths = collect_worktree_paths(root)?;
        let args = [
            OsString::from("check-attr"),
            OsString::from("-z"),
            OsString::from("--stdin"),
            OsString::from("filter"),
        ];
        let output = self.at_with_input(root, args, &paths)?;
        ensure_success(&output)?;
        ensure_no_active_filter_output(&output.stdout)
    }

    fn info_attributes_state(root: &Path) -> Result<(String, String), PortFailure> {
        let path = root.join(".git").join("info").join("attributes");
        match fs::read(path) {
            Ok(bytes) => {
                let metadata = fs::metadata(root.join(".git").join("info").join("attributes"))
                    .map_err(map_io_error)?;
                Ok((
                    crate::database::checksum_sha256(&bytes),
                    file_identity_signature(&metadata),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((
                crate::database::checksum_sha256(b"missing"),
                "missing".to_owned(),
            )),
            Err(error) => Err(map_io_error(error)),
        }
    }
}

/// Fixed argv for the shared read-only repository-status observation.
///
/// `--no-optional-locks` is a *global* Git option, so it must precede the
/// subcommand — that ordering is the whole point of pinning this array down
/// as a named constant with a regression test.
pub(crate) const READ_ONLY_STATUS_ARGUMENTS: [&str; 4] = [
    "--no-optional-locks",
    "status",
    "--porcelain=v1",
    "--untracked-files=all",
];

impl GitService for GitCliAdapter {
    fn is_available(&mut self) -> Result<bool, PortFailure> {
        Ok(self
            .output(std::iter::once(OsString::from("--version")))?
            .status
            .success())
    }

    fn inspect_project(&mut self, input: &Path) -> Result<ProjectInspection, PortFailure> {
        let canonical_input = canonical_local_directory(input)?;
        let root_output = self.at_str(&canonical_input, ["rev-parse", "--show-toplevel"])?;
        let (canonical_root, repository_kind, repository_status, git_common_dir) =
            if root_output.status.success() {
                let root =
                    canonical_local_directory(Path::new(trimmed_utf8(&root_output.stdout)?))?;
                let common = self.at_str(
                    &root,
                    ["rev-parse", "--path-format=absolute", "--git-common-dir"],
                )?;
                ensure_success(&common)?;
                let common = canonical_local_directory(Path::new(trimmed_utf8(&common.stdout)?))?;
                let expected_common = canonical_local_directory(&root.join(".git"))?;
                let git_dir =
                    self.at_str(&root, ["rev-parse", "--path-format=absolute", "--git-dir"])?;
                ensure_success(&git_dir)?;
                let git_dir = canonical_local_directory(Path::new(trimmed_utf8(&git_dir.stdout)?))?;
                let bare = self.at_str(&root, ["rev-parse", "--is-bare-repository"])?;
                ensure_success(&bare)?;
                if canonical_key(&common)? != canonical_key(&expected_common)?
                    || canonical_key(&git_dir)? != canonical_key(&common)?
                    || trimmed_utf8(&bare.stdout)? != "false"
                {
                    return Err(PortFailure::new(FailureCategory::Unsupported));
                }
                let status = self.repository_status(&root)?;
                (root, RepositoryKind::Git, Some(status), Some(common))
            } else {
                let bare = self.at_str(&canonical_input, ["rev-parse", "--is-bare-repository"])?;
                if bare.status.success() && trimmed_utf8(&bare.stdout)? == "true" {
                    return Err(PortFailure::new(FailureCategory::Unsupported));
                }
                if has_git_marker(&canonical_input) {
                    return Err(PortFailure::new(FailureCategory::Unsupported));
                }
                (canonical_input, RepositoryKind::NonGit, None, None)
            };
        let canonical_key = canonical_key(&canonical_root)?;
        let display_path = display_path(&canonical_root)?;
        let suggested_name = canonical_root
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| PortFailure::new(FailureCategory::InvalidInput))?
            .to_owned();
        let confirmation_token = confirmation_token(&canonical_key, repository_kind);
        Ok(ProjectInspection {
            canonical_root,
            canonical_key,
            display_path,
            suggested_name,
            confirmation_token,
            repository_kind,
            repository_status,
            git_common_dir,
        })
    }

    fn repository_status(&mut self, root: &Path) -> Result<RepositoryStatus, PortFailure> {
        // `--no-optional-locks` (a global option, so it must precede the
        // subcommand) stops `git status` from opportunistically refreshing
        // and rewriting the index, which is the only reason this read-only
        // observation would ever take `.git/index.lock`.
        //
        // This matters because `MergeConflictInspectionService` reaches this
        // helper on a 2-second UI poll while the task sits in
        // `MergeConflict` — and `commands::merge_abort` performs its
        // `git merge --abort` write against that same original checkout for
        // the whole of that window. Without this flag the two contend for
        // the index lock, and a merge abort that should have succeeded can
        // come back as `PostWriteUncertain`.
        //
        // Applied here rather than at the single merge-conflict call site
        // because every caller of this helper is a read-only observer
        // (project inspection, merge execution/continue/abort pre- and
        // post-write verification): none of them wants an index refresh as
        // a side effect. Mutation commands are untouched — they legitimately
        // take the index lock. Still a fixed argv array, still no shell
        // string.
        let status = self.at_str(root, READ_ONLY_STATUS_ARGUMENTS)?;
        ensure_success(&status)?;
        let branch = self.at_str(root, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let current_branch = branch
            .status
            .success()
            .then(|| trimmed_utf8(&branch.stdout).map(str::to_owned))
            .transpose()?;
        let head = self.at_str(root, ["rev-parse", "--verify", "HEAD"])?;
        let head_commit = head
            .status
            .success()
            .then(|| trimmed_utf8(&head.stdout).map(str::to_owned))
            .transpose()?;
        Ok(RepositoryStatus {
            clean: status.stdout.is_empty(),
            detached_head: current_branch.is_none() && head_commit.is_some(),
            current_branch,
            head_commit,
        })
    }

    fn validate_non_git_source(&mut self, root: &Path) -> Result<(), PortFailure> {
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).map_err(map_io_error)? {
                let entry = entry.map_err(map_io_error)?;
                if entry.file_name() == OsStr::new(".git") {
                    continue;
                }
                let kind = entry.file_type().map_err(map_io_error)?;
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if entry.file_name() == OsStr::new(".gitattributes") {
                    let content = fs::read_to_string(entry.path()).map_err(map_io_error)?;
                    if contains_active_filter_attribute(&content) {
                        return Err(PortFailure::new(FailureCategory::Unsupported));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_repository_source(
        &mut self,
        root: &Path,
        base_commit: &str,
    ) -> Result<RepositorySafetyToken, PortFailure> {
        validate_object_id(base_commit)?;
        self.reject_local_dangerous_config(root)?;
        let paths = self.at_str(root, ["ls-tree", "-r", "-z", "--name-only", base_commit])?;
        ensure_success(&paths)?;
        self.check_filter_attributes(root, Some(base_commit), &paths.stdout)?;
        let (info_attributes_digest, info_attributes_identity) = Self::info_attributes_state(root)?;
        Ok(RepositorySafetyToken {
            info_attributes_digest,
            info_attributes_identity,
        })
    }

    fn initialize_repository(&mut self, root: &Path) -> Result<(), PortFailure> {
        let inspection = self.inspect_project(root)?;
        if inspection.repository_kind != RepositoryKind::NonGit
            || canonical_key(&inspection.canonical_root)?
                != canonical_key(&canonical_local_directory(root)?)?
        {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        self.validate_non_git_source(root)?;
        fs::create_dir(root.join(".git")).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                PortFailure::new(FailureCategory::AlreadyExists)
            } else {
                map_io_error(error)
            }
        })?;
        let mut template = OsString::from("--template=");
        template.push(&self.control.template);
        ensure_success(&self.at(root, [OsString::from("init"), template])?)
    }

    fn has_commit_author(&mut self, root: &Path) -> Result<bool, PortFailure> {
        for key in ["user.name", "user.email"] {
            let local = self.author_output(root, key, true)?;
            let present = local.status.success() && !trimmed_utf8(&local.stdout)?.is_empty();
            let global = if present {
                true
            } else {
                let global = self.author_output(root, key, false)?;
                global.status.success() && !trimmed_utf8(&global.stdout)?.is_empty()
            };
            if !global {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn create_initial_snapshot(&mut self, root: &Path) -> Result<String, PortFailure> {
        self.validate_non_git_source(root)?;
        self.reject_local_dangerous_config(root)?;
        reject_active_info_attributes(root)?;
        self.check_worktree_filter_attributes(root)?;
        ensure_success(&self.at_str(root, ["add", "-A", "--", "."])?)?;
        let indexed = self.at_str(root, ["ls-files", "-z"])?;
        ensure_success(&indexed)?;
        self.check_filter_attributes(root, None, &indexed.stdout)?;
        ensure_success(&self.at_str(
            root,
            ["commit", "--allow-empty", "-m", INITIAL_SNAPSHOT_MESSAGE],
        )?)?;
        let head = self.at_str(root, ["rev-parse", "--verify", "HEAD"])?;
        ensure_success(&head)?;
        let commit = trimmed_utf8(&head.stdout)?;
        validate_object_id(commit)?;
        Ok(commit.to_owned())
    }

    fn create_task_worktree(
        &mut self,
        root: &Path,
        branch: &str,
        base_commit: &str,
        worktree: &Path,
        safety: &RepositorySafetyToken,
    ) -> Result<WorktreeCreationOutcome, PortFailure> {
        validate_branch(branch)?;
        validate_object_id(base_commit)?;
        let status = self.repository_status(root)?;
        if !status.ready_for_isolation() || status.head_commit.as_deref() != Some(base_commit) {
            return Ok(WorktreeCreationOutcome::Uncertain);
        }
        if &self.validate_repository_source(root, base_commit)? != safety {
            return Ok(WorktreeCreationOutcome::Uncertain);
        }
        if fs::symlink_metadata(worktree).is_ok() || branch_commit(self, root, branch)?.is_some() {
            return Err(PortFailure::new(FailureCategory::AlreadyExists));
        }
        let output = self.at(
            root,
            [
                OsString::from("worktree"),
                OsString::from("add"),
                OsString::from("--lock"),
                OsString::from("-b"),
                OsString::from(branch),
                worktree.as_os_str().to_owned(),
                OsString::from(base_commit),
            ],
        )?;
        let (digest, identity) = Self::info_attributes_state(root)?;
        let info_unchanged =
            digest == safety.info_attributes_digest && identity == safety.info_attributes_identity;
        if output.status.success() && info_unchanged {
            return Ok(WorktreeCreationOutcome::Created);
        }
        if branch_commit(self, root, branch)?.is_none()
            && fs::symlink_metadata(worktree)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            Ok(WorktreeCreationOutcome::NoEffect)
        } else {
            Ok(WorktreeCreationOutcome::Uncertain)
        }
    }

    fn verify_task_worktree(
        &mut self,
        root: &Path,
        branch: &str,
        base_commit: &str,
        worktree: &Path,
    ) -> Result<bool, PortFailure> {
        if !worktree.is_dir() || branch_commit(self, root, branch)?.as_deref() != Some(base_commit)
        {
            return Ok(false);
        }
        let head = self.at_str(worktree, ["rev-parse", "--verify", "HEAD"])?;
        let actual_branch =
            self.at_str(worktree, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let reported_root = self.at_str(worktree, ["rev-parse", "--show-toplevel"])?;
        let status = self.at_str(
            worktree,
            ["status", "--porcelain=v1", "--untracked-files=all"],
        )?;
        let common = self.at_str(
            worktree,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let git_dir = self.at_str(
            worktree,
            ["rev-parse", "--path-format=absolute", "--git-dir"],
        )?;
        if !head.status.success()
            || !actual_branch.status.success()
            || !reported_root.status.success()
            || !status.status.success()
            || !common.status.success()
            || !git_dir.status.success()
            || !status.stdout.is_empty()
            || trimmed_utf8(&head.stdout)? != base_commit
            || trimmed_utf8(&actual_branch.stdout)? != branch
        {
            return Ok(false);
        }
        let actual_root =
            canonical_local_directory(Path::new(trimmed_utf8(&reported_root.stdout)?))?;
        let expected_root = canonical_local_directory(worktree)?;
        let actual_common = canonical_local_directory(Path::new(trimmed_utf8(&common.stdout)?))?;
        let expected_common = canonical_local_directory(&root.join(".git"))?;
        let actual_git_dir = canonical_local_directory(Path::new(trimmed_utf8(&git_dir.stdout)?))?;
        Ok(
            canonical_key(&actual_root)? == canonical_key(&expected_root)?
                && canonical_key(&actual_common)? == canonical_key(&expected_common)?
                && actual_git_dir.starts_with(expected_common.join("worktrees")),
        )
    }
}

impl GitCliAdapter {
    pub(crate) fn verify_task_worktree_with_changes(
        &mut self,
        root: &Path,
        branch: &str,
        base_commit: &str,
        worktree: &Path,
    ) -> Result<bool, PortFailure> {
        if !worktree.is_dir() || branch_commit(self, root, branch)?.as_deref() != Some(base_commit)
        {
            return Ok(false);
        }
        let head = self.at_str(worktree, ["rev-parse", "--verify", "HEAD"])?;
        let actual_branch =
            self.at_str(worktree, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let reported_root = self.at_str(worktree, ["rev-parse", "--show-toplevel"])?;
        let common = self.at_str(
            worktree,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let git_dir = self.at_str(
            worktree,
            ["rev-parse", "--path-format=absolute", "--git-dir"],
        )?;
        if !head.status.success()
            || !actual_branch.status.success()
            || !reported_root.status.success()
            || !common.status.success()
            || !git_dir.status.success()
            || trimmed_utf8(&head.stdout)? != base_commit
            || trimmed_utf8(&actual_branch.stdout)? != branch
        {
            return Ok(false);
        }
        let actual_root =
            canonical_local_directory(Path::new(trimmed_utf8(&reported_root.stdout)?))?;
        let expected_root = canonical_local_directory(worktree)?;
        let actual_common = canonical_local_directory(Path::new(trimmed_utf8(&common.stdout)?))?;
        let expected_common = canonical_local_directory(&root.join(".git"))?;
        let actual_git_dir = canonical_local_directory(Path::new(trimmed_utf8(&git_dir.stdout)?))?;
        Ok(
            canonical_key(&actual_root)? == canonical_key(&expected_root)?
                && canonical_key(&actual_common)? == canonical_key(&expected_common)?
                && actual_git_dir.starts_with(expected_common.join("worktrees")),
        )
    }

    fn capture_candidate_git(
        &mut self,
        arguments: Vec<OsString>,
    ) -> Result<BoundedCaptureOutcome, PortFailure> {
        self.ensure_control_paths()?;
        self.validate_runtime()?;
        self.validate_control_paths()?;
        let mut command = Command::new(self.runtime.executable());
        command
            .env_clear()
            .current_dir(&self.control.home)
            .args(self.common_arguments())
            .args(arguments);
        configure_controlled_environment(&mut command, &self.runtime, &self.control)?;
        capture_bounded_stdout(command, DIFF_MAX_BYTES, DIFF_TIMEOUT)
    }
}

/// Reads the current worktree diff via the same trusted Git runtime and
/// `env_clear`'d control-path machinery every other `GitCliAdapter` command
/// uses (see `common_arguments`/`configure_controlled_environment`), but
/// through its own bounded/time-limited capture (`capture_bounded_stdout`)
/// rather than `output`/`output_with_input`'s unbounded
/// `wait_with_output`. Deliberately a separate trait ([`WorktreeDiffPort`])
/// rather than a new [`GitService`] method — see that trait's doc comment.
impl WorktreeDiffPort for GitCliAdapter {
    fn current_diff(&mut self, worktree: &Path) -> Result<WorktreeDiffOutcome, PortFailure> {
        if !worktree.is_absolute() {
            return Err(PortFailure::new(FailureCategory::InvalidInput));
        }
        self.ensure_control_paths()?;
        self.validate_runtime()?;
        self.validate_control_paths()?;
        let mut command = Command::new(self.runtime.executable());
        command
            .env_clear()
            .current_dir(&self.control.home)
            .args(self.common_arguments())
            .args(diff_arguments(worktree));
        configure_controlled_environment(&mut command, &self.runtime, &self.control)?;
        classify_capture(capture_bounded_stdout(
            command,
            DIFF_MAX_BYTES,
            DIFF_TIMEOUT,
        )?)
    }
}

/// Read-only approval candidate. Its verifier intentionally permits task
/// worktree changes, while preserving the clean-only GitService verifier for
/// planning and review flows.
impl CommitCandidatePort for GitCliAdapter {
    fn current_commit_candidate(
        &mut self,
        root: &Path,
        base_branch: &str,
        task_branch: &str,
        base_commit: &str,
        worktree: &Path,
    ) -> Result<CommitCandidateOutcome, PortFailure> {
        validate_base_branch(base_branch)?;
        validate_branch(task_branch)?;
        validate_object_id(base_commit)?;
        if !root.is_absolute() || !worktree.is_absolute() {
            return Err(PortFailure::new(FailureCategory::InvalidInput));
        }
        let source = self.repository_status(root)?;
        if !source.clean
            || source.current_branch.as_deref() != Some(base_branch)
            || source.head_commit.as_deref() != Some(base_commit)
            || !self.verify_task_worktree_with_changes(root, task_branch, base_commit, worktree)?
        {
            return Err(PortFailure::new(FailureCategory::Conflict));
        }
        let tracked = match self.capture_candidate_git(diff_arguments(worktree))? {
            BoundedCaptureOutcome::Success(bytes) => String::from_utf8(bytes)
                .map_err(|_| PortFailure::new(FailureCategory::InvalidInput))?,
            BoundedCaptureOutcome::ExitFailure => {
                return Err(PortFailure::new(FailureCategory::Conflict));
            }
            BoundedCaptureOutcome::TooLarge => {
                return Ok(CommitCandidateOutcome::CandidateTooLarge);
            }
            BoundedCaptureOutcome::TimedOut => return Ok(CommitCandidateOutcome::TimedOut),
            BoundedCaptureOutcome::Uncertain => return Ok(CommitCandidateOutcome::Uncertain),
        };
        let listed = match self.capture_candidate_git(untracked_arguments(worktree))? {
            BoundedCaptureOutcome::Success(bytes) => bytes,
            BoundedCaptureOutcome::ExitFailure => {
                return Err(PortFailure::new(FailureCategory::Conflict));
            }
            BoundedCaptureOutcome::TooLarge => {
                return Ok(CommitCandidateOutcome::CandidateTooLarge);
            }
            BoundedCaptureOutcome::TimedOut => return Ok(CommitCandidateOutcome::TimedOut),
            BoundedCaptureOutcome::Uncertain => return Ok(CommitCandidateOutcome::Uncertain),
        };
        let paths = canonical_untracked_paths(&listed)?;
        if tracked.is_empty() && paths.is_empty() {
            return Ok(CommitCandidateOutcome::NoChanges);
        }
        let worktree = canonical_local_directory(worktree)?;
        let mut canonical = String::from("--- ChatOMS tracked diff ---\n");
        append_candidate(&mut canonical, &tracked)?;
        for relative in paths {
            let candidate = checked_untracked_file(&worktree, &relative)?;
            let bytes = fs::read(candidate).map_err(map_io_error)?;
            let content = std::str::from_utf8(&bytes)
                .map_err(|_| PortFailure::new(FailureCategory::InvalidInput))?;
            append_candidate(&mut canonical, "\n--- ChatOMS untracked file: ")?;
            append_candidate(&mut canonical, &relative)?;
            append_candidate(&mut canonical, "\n")?;
            append_candidate(&mut canonical, content)?;
            append_candidate(&mut canonical, "\n--- End ChatOMS untracked file ---\n")?;
        }
        let digest = Sha256::digest(canonical.as_bytes());
        let mut hash = [0_u8; 32];
        hash.copy_from_slice(&digest);
        Ok(CommitCandidateOutcome::Candidate(CommitCandidate::new(
            canonical,
            DiffContentHash::from_digest_bytes(hash),
        )))
    }
}

fn untracked_arguments(worktree: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("ls-files"),
        OsString::from("--others"),
        OsString::from("--exclude-standard"),
        OsString::from("-z"),
        OsString::from("--"),
        OsString::from("."),
    ]
}

fn canonical_untracked_paths(listed: &[u8]) -> Result<Vec<String>, PortFailure> {
    let mut paths = Vec::new();
    for raw in listed
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let path = std::str::from_utf8(raw)
            .map_err(|_| PortFailure::new(FailureCategory::InvalidInput))?;
        let relative = Path::new(path);
        if path.is_empty()
            || path.contains(['\n', '\r'])
            || relative.is_absolute()
            || relative.components().any(
                |part| !matches!(part, Component::Normal(value) if !value.to_string_lossy().eq_ignore_ascii_case(".git")),
            )
        {
            return Err(PortFailure::new(FailureCategory::InvalidInput));
        }
        paths.push(path.to_owned());
    }
    paths.sort_unstable();
    if paths.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    Ok(paths)
}

fn checked_untracked_file(worktree: &Path, relative: &str) -> Result<PathBuf, PortFailure> {
    let path = worktree.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(map_io_error)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    let canonical = fs::canonicalize(&path).map_err(map_io_error)?;
    if !canonical.starts_with(worktree) {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    Ok(canonical)
}

fn append_candidate(target: &mut String, segment: &str) -> Result<(), PortFailure> {
    if target
        .len()
        .checked_add(segment.len())
        .is_none_or(|size| size > DIFF_MAX_BYTES)
    {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    target.push_str(segment);
    Ok(())
}

/// Fixed, non-caller-influenced argv for a bounded current-worktree diff:
/// `-C <worktree> diff --no-color --no-ext-diff --no-textconv HEAD -- .`.
/// `--no-ext-diff`/`--no-textconv` stop a repository-local `diff.*`
/// driver/textconv config (or a `GIT_EXTERNAL_DIFF` the controlled
/// environment already strips) from running arbitrary content-dependent
/// code. `diff HEAD` (rather than separate `diff` and `diff --cached`
/// calls) reports the union of staged and unstaged changes against the
/// worktree's own current commit in one call. The trailing `-- .` pins the
/// diff to files inside `worktree` even though `-C worktree` already scopes
/// it there, as defense in depth against any future argument added before
/// it. No revision or path ever comes from a caller.
fn diff_arguments(worktree: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-C"),
        worktree.as_os_str().to_owned(),
        OsString::from("diff"),
        OsString::from("--no-color"),
        OsString::from("--no-ext-diff"),
        OsString::from("--no-textconv"),
        OsString::from("HEAD"),
        OsString::from("--"),
        OsString::from("."),
    ]
}

/// Raw disposition of a bounded, time-limited process capture, before it
/// has been reduced to the safe [`WorktreeDiffOutcome`]/[`PortFailure`]
/// vocabulary by [`classify_capture`]. Kept separate so the bounded-capture
/// mechanics (spawn, drain stdout/stderr, enforce a byte cap and a
/// deadline) can be unit-tested against an arbitrary fixture process,
/// independent of the trusted Git runtime.
pub(crate) enum BoundedCaptureOutcome {
    Success(Vec<u8>),
    ExitFailure,
    TooLarge,
    TimedOut,
    Uncertain,
}

/// Spawns `command` (already fully configured by the caller — executable,
/// args, cwd, environment) with piped stdout/stderr, and reads stdout on a
/// background thread that stops as soon as `max_bytes` would be exceeded
/// (so an oversized diff is never buffered in full) while stderr is drained
/// and discarded purely to prevent pipe backpressure. The main thread polls
/// `try_wait` against `timeout`; if the deadline passes first, the child is
/// killed. A byte-bound breach takes priority over a clean exit or a
/// timeout if both become true, since the content is unusable either way.
fn capture_bounded_stdout(
    mut command: Command,
    max_bytes: usize,
    timeout: Duration,
) -> Result<BoundedCaptureOutcome, PortFailure> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_spawn_error)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| PortFailure::new(FailureCategory::Internal))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| PortFailure::new(FailureCategory::Internal))?;

    let stderr_drain = thread::spawn(move || {
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    });

    let too_large = Arc::new(AtomicBool::new(false));
    let too_large_reader = Arc::clone(&too_large);
    let stdout_reader = thread::spawn(move || -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if buffer.len() + read > max_bytes {
                        too_large_reader.store(true, Ordering::SeqCst);
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
        }
        buffer
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if too_large.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    timed_out = !too_large.load(Ordering::SeqCst);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                thread::sleep(DIFF_POLL_INTERVAL);
            }
            Err(_) => break None,
        }
    };

    let _ = stderr_drain.join();
    let Ok(stdout_bytes) = stdout_reader.join() else {
        return Ok(BoundedCaptureOutcome::Uncertain);
    };

    if too_large.load(Ordering::SeqCst) {
        return Ok(BoundedCaptureOutcome::TooLarge);
    }
    let Some(status) = status else {
        return Ok(if timed_out {
            BoundedCaptureOutcome::TimedOut
        } else {
            BoundedCaptureOutcome::Uncertain
        });
    };
    if !status.success() {
        return Ok(BoundedCaptureOutcome::ExitFailure);
    }
    Ok(BoundedCaptureOutcome::Success(stdout_bytes))
}

/// Reduces a [`BoundedCaptureOutcome`] to the port's safe vocabulary. A
/// non-zero Git exit is treated as a genuine command failure
/// ([`FailureCategory::Conflict`], matching this file's existing
/// `ensure_success`), and non-UTF-8 stdout as malformed input
/// ([`FailureCategory::InvalidInput`], matching `trimmed_utf8`) — both
/// `Err`, not an outcome variant, since neither is a safe-to-classify
/// disposition of a well-formed run.
fn classify_capture(outcome: BoundedCaptureOutcome) -> Result<WorktreeDiffOutcome, PortFailure> {
    match outcome {
        BoundedCaptureOutcome::TooLarge => Ok(WorktreeDiffOutcome::DiffTooLarge),
        BoundedCaptureOutcome::TimedOut => Ok(WorktreeDiffOutcome::TimedOut),
        BoundedCaptureOutcome::Uncertain => Ok(WorktreeDiffOutcome::Uncertain),
        BoundedCaptureOutcome::ExitFailure => Err(PortFailure::new(FailureCategory::Conflict)),
        BoundedCaptureOutcome::Success(bytes) if bytes.is_empty() => {
            Ok(WorktreeDiffOutcome::NoChanges)
        }
        BoundedCaptureOutcome::Success(bytes) => String::from_utf8(bytes)
            .map(|text| WorktreeDiffOutcome::Diff(WorktreeDiff::new(text)))
            .map_err(|_| PortFailure::new(FailureCategory::InvalidInput)),
    }
}

fn git_control_paths(root: &Path) -> GitControlPaths {
    GitControlPaths {
        home: root.join("home"),
        hooks: root.join("hooks-empty"),
        template: root.join("template-empty"),
        global_config: root.join("global-empty.config"),
        global_attributes: root.join("attributes-empty"),
    }
}

fn prepare_control_paths(control: &GitControlPaths) -> Result<(), PortFailure> {
    let root = control
        .home
        .parent()
        .ok_or_else(|| PortFailure::new(FailureCategory::StorageInsecure))?;
    ensure_control_directory(root)?;
    ensure_control_directory(&control.home)?;
    ensure_control_directory(&control.hooks)?;
    ensure_control_directory(&control.template)?;
    for file in [&control.global_config, &control.global_attributes] {
        match OpenOptions::new().write(true).create_new(true).open(file) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(map_io_error(error)),
        }
    }
    Ok(())
}

fn ensure_control_directory(path: &Path) -> Result<(), PortFailure> {
    ensure_supported_directory(path).map_err(|_| PortFailure::new(FailureCategory::StorageInsecure))
}

fn capture_control_identity(
    control: &GitControlPaths,
) -> Result<Vec<(PathBuf, String)>, PortFailure> {
    [
        &control.home,
        &control.hooks,
        &control.template,
        &control.global_config,
        &control.global_attributes,
    ]
    .into_iter()
    .map(|path| {
        fs::metadata(path)
            .map(|metadata| (path.clone(), file_identity_signature(&metadata)))
            .map_err(map_io_error)
    })
    .collect()
}

fn capture_control_directory_identity(
    control: &GitControlPaths,
) -> Result<Vec<DirectoryIdentity>, PortFailure> {
    [&control.home, &control.hooks, &control.template]
        .into_iter()
        .map(|path| {
            supported_directory_identity(path)
                .map_err(|_| PortFailure::new(FailureCategory::StorageInsecure))
        })
        .collect()
}

fn reject_active_info_attributes(root: &Path) -> Result<(), PortFailure> {
    let path = root.join(".git").join("info").join("attributes");
    match fs::read_to_string(path) {
        Ok(content) if contains_active_filter_attribute(&content) => {
            Err(PortFailure::new(FailureCategory::Unsupported))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(map_io_error(error)),
    }
}

fn collect_worktree_paths(root: &Path) -> Result<Vec<u8>, PortFailure> {
    let mut output = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(map_io_error)? {
            let entry = entry.map_err(map_io_error)?;
            if entry.path() == root.join(".git") {
                continue;
            }
            let kind = entry.file_type().map_err(map_io_error)?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| PortFailure::new(FailureCategory::Conflict))?
                    .to_str()
                    .ok_or_else(|| PortFailure::new(FailureCategory::InvalidInput))?
                    .replace('\\', "/");
                output.extend_from_slice(relative.as_bytes());
                output.push(0);
            } else {
                return Err(PortFailure::new(FailureCategory::Unsupported));
            }
        }
    }
    Ok(output)
}

fn ensure_no_active_filter_output(bytes: &[u8]) -> Result<(), PortFailure> {
    for triple in bytes.split(|byte| *byte == 0).collect::<Vec<_>>().chunks(3) {
        if triple.len() < 3 || triple[0].is_empty() {
            continue;
        }
        let value = std::str::from_utf8(triple[2])
            .map_err(|_| PortFailure::new(FailureCategory::InvalidInput))?;
        if !matches!(value, "unspecified" | "unset") {
            return Err(PortFailure::new(FailureCategory::Unsupported));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn file_identity_signature(metadata: &fs::Metadata) -> String {
    use std::os::windows::fs::MetadataExt;
    format!(
        "{:016x}:{:016x}:{}:{}",
        metadata.creation_time(),
        metadata.last_write_time(),
        metadata.file_size(),
        metadata.file_attributes()
    )
}

#[cfg(not(windows))]
fn file_identity_signature(metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!(
        "{:016x}:{:016x}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.size()
    )
}

fn configure_controlled_environment(
    command: &mut Command,
    runtime: &TrustedGitRuntime,
    control: &GitControlPaths,
) -> Result<(), PortFailure> {
    command
        .env("PATH", controlled_path(runtime)?)
        .env("GIT_EXEC_PATH", runtime.exec_path())
        .env("SystemRoot", runtime.system_root())
        .env("WINDIR", runtime.system_root())
        .env("TEMP", &control.home)
        .env("TMP", &control.home)
        .env("HOME", &control.home)
        .env("XDG_CONFIG_HOME", &control.home)
        .env("GIT_CONFIG_GLOBAL", &control.global_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .env("LANG", "C");
    Ok(())
}

fn controlled_path(runtime: &TrustedGitRuntime) -> Result<OsString, PortFailure> {
    let paths = vec![
        runtime.cmd().to_path_buf(),
        runtime.bin().to_path_buf(),
        runtime.system_directory().to_path_buf(),
    ];
    env::join_paths(paths).map_err(|_| PortFailure::new(FailureCategory::InvalidInput))
}

fn discover_trusted_git_runtime() -> Result<TrustedGitRuntime, PortFailure> {
    TrustedGitRuntime::discover().map_err(|_| PortFailure::new(FailureCategory::Unsupported))
}

fn validate_trusted_runtime(runtime: &TrustedGitRuntime) -> Result<(), PortFailure> {
    runtime
        .validate()
        .map_err(|_| PortFailure::new(FailureCategory::Conflict))
}

fn branch_commit(
    adapter: &mut GitCliAdapter,
    root: &Path,
    branch: &str,
) -> Result<Option<String>, PortFailure> {
    validate_branch(branch)?;
    let output = adapter.at(
        root,
        [
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from(format!("refs/heads/{branch}")),
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let commit = trimmed_utf8(&output.stdout)?;
    validate_object_id(commit)?;
    Ok(Some(commit.to_owned()))
}

fn has_git_marker(path: &Path) -> bool {
    path.ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
}

fn contains_active_filter_attribute(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return false;
        }
        line.split_ascii_whitespace()
            .skip(1)
            .any(|attribute| attribute == "filter" || attribute.starts_with("filter="))
    })
}

fn ensure_success(output: &Output) -> Result<(), PortFailure> {
    if output.status.success() {
        Ok(())
    } else {
        Err(PortFailure::new(FailureCategory::Conflict))
    }
}

fn trimmed_utf8(bytes: &[u8]) -> Result<&str, PortFailure> {
    std::str::from_utf8(bytes)
        .map(str::trim)
        .map_err(|_| PortFailure::new(FailureCategory::InvalidInput))
}

fn canonical_local_directory(path: &Path) -> Result<PathBuf, PortFailure> {
    if !path.is_absolute() || is_network_path(path) {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    let metadata = fs::metadata(path).map_err(map_io_error)?;
    if !metadata.is_dir() {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    let canonical = fs::canonicalize(path).map_err(map_io_error)?;
    if is_network_path(&canonical) {
        return Err(PortFailure::new(FailureCategory::Unsupported));
    }
    Ok(canonical)
}

#[cfg(windows)]
fn is_network_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(_)
            )
    )
}

#[cfg(not(windows))]
fn is_network_path(_path: &Path) -> bool {
    false
}

fn canonical_key(path: &Path) -> Result<String, PortFailure> {
    let value = path
        .to_str()
        .ok_or_else(|| PortFailure::new(FailureCategory::InvalidInput))?;
    #[cfg(windows)]
    let value = value
        .strip_prefix("\\\\?\\")
        .unwrap_or(value)
        .replace('\\', "/")
        .to_lowercase();
    #[cfg(not(windows))]
    let value = value.to_owned();
    Ok(value)
}

fn display_path(path: &Path) -> Result<String, PortFailure> {
    #[cfg(windows)]
    {
        if let Some(profile) = env::var_os("USERPROFILE")
            && let Ok(profile) = fs::canonicalize(profile)
            && let (Ok(path_key), Ok(profile_key)) = (canonical_key(path), canonical_key(&profile))
            && (path_key == profile_key || path_key.starts_with(&(profile_key.clone() + "/")))
        {
            let suffix = path_key.strip_prefix(&profile_key).unwrap_or_default();
            return Ok(format!("%USERPROFILE%{}", suffix.replace('/', "\\")));
        }
    }
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let tail = components.iter().rev().take(2).copied().collect::<Vec<_>>();
    if tail.is_empty() {
        return Err(PortFailure::new(FailureCategory::InvalidInput));
    }
    Ok(format!(
        "…\\{}",
        tail.into_iter().rev().collect::<Vec<_>>().join("\\")
    ))
}

fn confirmation_token(canonical_key: &str, kind: RepositoryKind) -> String {
    let suffix = match kind {
        RepositoryKind::Git => b"\0git".as_slice(),
        RepositoryKind::NonGit => b"\0non-git".as_slice(),
    };
    let mut material = Vec::with_capacity(canonical_key.len() + suffix.len());
    material.extend_from_slice(canonical_key.as_bytes());
    material.extend_from_slice(suffix);
    crate::database::checksum_sha256(&material)
}

fn validate_branch(branch: &str) -> Result<(), PortFailure> {
    if branch.starts_with("ai-task/")
        && branch.len() > "ai-task/".len()
        && branch.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'/')
        })
    {
        Ok(())
    } else {
        Err(PortFailure::new(FailureCategory::InvalidInput))
    }
}

fn validate_base_branch(branch: &str) -> Result<(), PortFailure> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.starts_with('/')
        || branch.ends_with('/')
        || branch.ends_with('.')
        || branch.contains("..")
        || branch.contains("@{")
        || branch.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
    {
        Err(PortFailure::new(FailureCategory::InvalidInput))
    } else {
        Ok(())
    }
}

fn validate_object_id(value: &str) -> Result<(), PortFailure> {
    if matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(PortFailure::new(FailureCategory::InvalidInput))
    }
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x0000_0400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use chatoms_ports::error::CategorizedFailure;

    /// `--no-optional-locks` only takes effect as a global option, i.e.
    /// before the subcommand. If it ever drifts after `status`, Git silently
    /// treats it as an unknown `status` flag and the read-only observation
    /// starts taking `.git/index.lock` again, contending with the
    /// merge-conflict writes that run against the same checkout.
    #[test]
    fn read_only_status_argv_passes_no_optional_locks_before_the_subcommand() {
        assert_eq!(
            READ_ONLY_STATUS_ARGUMENTS,
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ]
        );
        let flag = READ_ONLY_STATUS_ARGUMENTS
            .iter()
            .position(|argument| *argument == "--no-optional-locks")
            .expect("the read-only status argv must carry --no-optional-locks");
        let subcommand = READ_ONLY_STATUS_ARGUMENTS
            .iter()
            .position(|argument| *argument == "status")
            .expect("the read-only status argv must invoke `status`");
        assert!(
            flag < subcommand,
            "--no-optional-locks is a global option and must precede the subcommand"
        );
    }

    /// Every element is a separate argv entry: no element may smuggle in a
    /// space-separated pair, which is how a fixed argv array degrades into
    /// something shell-like.
    #[test]
    fn read_only_status_argv_has_no_combined_or_empty_arguments() {
        for argument in READ_ONLY_STATUS_ARGUMENTS {
            assert!(!argument.is_empty());
            assert!(
                !argument.contains(char::is_whitespace),
                "argv entries must stay separate"
            );
        }
    }

    // These tests exercise `capture_bounded_stdout`/`classify_capture`
    // against fixture `cmd.exe` processes, never a real (or even fake) Git
    // binary — the trusted-runtime-gated `WorktreeDiffPort::current_diff`
    // itself needs a genuine signed Git for Windows install to spawn at
    // all, so its own argv construction is covered separately by
    // `diff_arguments_are_fixed_and_scoped_to_worktree` below, a pure
    // function with no process involved.

    fn fixture_command(script: &str) -> Command {
        let mut command = Command::new("cmd");
        command.args(["/C", script]);
        command
    }

    #[test]
    fn diff_arguments_are_fixed_and_scoped_to_worktree() {
        let worktree = Path::new(r"C:\ChatOMS\worktrees\project\task");
        let arguments = diff_arguments(worktree);
        assert_eq!(
            arguments,
            vec![
                OsString::from("-C"),
                OsString::from(worktree),
                OsString::from("diff"),
                OsString::from("--no-color"),
                OsString::from("--no-ext-diff"),
                OsString::from("--no-textconv"),
                OsString::from("HEAD"),
                OsString::from("--"),
                OsString::from("."),
            ]
        );
    }

    #[test]
    fn capture_returns_success_bytes_for_a_clean_exit() {
        let outcome = capture_bounded_stdout(
            fixture_command("echo hello"),
            DIFF_MAX_BYTES,
            Duration::from_secs(5),
        )
        .expect("capture succeeds");
        let BoundedCaptureOutcome::Success(bytes) = outcome else {
            panic!("expected Success, got a different outcome");
        };
        assert!(String::from_utf8(bytes).expect("utf8").contains("hello"));
    }

    #[test]
    fn capture_returns_empty_success_bytes_for_no_output() {
        let outcome = capture_bounded_stdout(
            fixture_command("exit 0"),
            DIFF_MAX_BYTES,
            Duration::from_secs(5),
        )
        .expect("capture succeeds");
        let BoundedCaptureOutcome::Success(bytes) = outcome else {
            panic!("expected Success, got a different outcome");
        };
        assert!(bytes.is_empty());
    }

    #[test]
    fn capture_reports_exit_failure_without_needing_output_content() {
        let outcome = capture_bounded_stdout(
            fixture_command("exit 3"),
            DIFF_MAX_BYTES,
            Duration::from_secs(5),
        )
        .expect("capture succeeds");
        assert!(matches!(outcome, BoundedCaptureOutcome::ExitFailure));
    }

    #[test]
    fn capture_stops_reading_once_the_byte_bound_is_exceeded() {
        let outcome = capture_bounded_stdout(
            fixture_command("echo 0123456789012345678901234567890123456789"),
            16,
            Duration::from_secs(5),
        )
        .expect("capture succeeds");
        assert!(matches!(outcome, BoundedCaptureOutcome::TooLarge));
    }

    #[test]
    fn capture_kills_and_reports_timeout_for_a_hanging_process() {
        // Spawned directly (no `cmd /C` wrapper), matching how the
        // production adapter spawns `git.exe` itself: a single process with
        // no grandchild. `child.kill()` only terminates the process it was
        // given, not any children it may have spawned (this is exactly why
        // `StdProcessRunner::run_streaming` elsewhere in this crate needs
        // `taskkill /T /F`) — a `cmd /C ping ...` fixture here would leave
        // an orphaned `ping.exe` holding the pipe open for the full
        // duration, which does not reflect how `current_diff` actually
        // spawns Git.
        let started = Instant::now();
        let mut command = Command::new("ping");
        command.args(["127.0.0.1", "-n", "30"]);
        let outcome = capture_bounded_stdout(command, DIFF_MAX_BYTES, Duration::from_millis(150))
            .expect("capture succeeds");
        assert!(matches!(outcome, BoundedCaptureOutcome::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout should cut the hang short, not wait for the full ping duration"
        );
    }

    #[test]
    fn classify_maps_too_large_and_timeout_and_uncertain_to_outcomes_not_errors() {
        assert_eq!(
            classify_capture(BoundedCaptureOutcome::TooLarge).expect("ok"),
            WorktreeDiffOutcome::DiffTooLarge
        );
        assert_eq!(
            classify_capture(BoundedCaptureOutcome::TimedOut).expect("ok"),
            WorktreeDiffOutcome::TimedOut
        );
        assert_eq!(
            classify_capture(BoundedCaptureOutcome::Uncertain).expect("ok"),
            WorktreeDiffOutcome::Uncertain
        );
    }

    #[test]
    fn classify_maps_exit_failure_to_a_typed_error_without_raw_content() {
        let error =
            classify_capture(BoundedCaptureOutcome::ExitFailure).expect_err("exit failure errs");
        assert_eq!(error.category(), FailureCategory::Conflict);
    }

    #[test]
    fn classify_maps_empty_success_to_no_changes() {
        assert_eq!(
            classify_capture(BoundedCaptureOutcome::Success(Vec::new())).expect("ok"),
            WorktreeDiffOutcome::NoChanges
        );
    }

    #[test]
    fn classify_maps_non_utf8_success_to_a_typed_error_without_raw_bytes() {
        let error = classify_capture(BoundedCaptureOutcome::Success(vec![0xFF, 0xFE]))
            .expect_err("non-UTF-8 bytes err");
        assert_eq!(error.category(), FailureCategory::InvalidInput);
    }

    #[test]
    fn untracked_paths_are_sorted_and_reject_git_or_non_utf8_entries() {
        assert_eq!(
            canonical_untracked_paths(b"z.txt\0a.txt\0").expect("valid paths"),
            vec!["a.txt", "z.txt"]
        );
        for invalid in [
            b".git/config\0".as_slice(),
            b".GIT/config\0".as_slice(),
            b"../escape\0".as_slice(),
            &[0xff, 0][..],
        ] {
            assert_eq!(
                canonical_untracked_paths(invalid)
                    .expect_err("unsafe path must fail closed")
                    .category(),
                FailureCategory::InvalidInput
            );
        }
    }

    #[test]
    fn candidate_append_enforces_the_shared_diff_bound() {
        let mut candidate = "x".repeat(DIFF_MAX_BYTES);
        assert_eq!(
            append_candidate(&mut candidate, "y")
                .expect_err("combined candidate exceeds bound")
                .category(),
            FailureCategory::InvalidInput
        );
    }

    #[test]
    fn classify_maps_nonempty_utf8_success_to_diff_text() {
        let outcome = classify_capture(BoundedCaptureOutcome::Success(
            b"--- a/f\n+++ b/f\n".to_vec(),
        ))
        .expect("ok");
        let WorktreeDiffOutcome::Diff(diff) = outcome else {
            panic!("expected a Diff outcome");
        };
        assert_eq!(diff.text(), "--- a/f\n+++ b/f\n");
    }
}
