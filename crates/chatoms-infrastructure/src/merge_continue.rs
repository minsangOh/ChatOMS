//! `git merge --continue` on the original checkout of a task's
//! `MergeConflict`, gated on an immutable, confirmed manual-resolution
//! digest (see `chatoms_ports::manual_merge_resolution`). This is the only
//! Git write this adapter performs: no abort, reset, or automatic conflict
//! resolution.
//!
//! The write-time prewrite gate reuses
//! [`ManualMergeResolutionCandidatePort::resolution_candidate`] to
//! re-verify every identity/topology/residue/configuration/status
//! precondition fresh, immediately before spawning — never trusting the
//! caller's earlier read alone.

use std::{ffi::OsStr, path::PathBuf};

use chatoms_ports::{
    git::GitService,
    manual_merge_resolution::{
        ManualMergeResolutionCandidatePort, ManualMergeResolutionCandidateRequest,
        ManualResolutionCandidateOutcome,
    },
    merge_continue::{MergeContinueOutcome, MergeContinuePort, MergeContinueRequest},
};

use crate::{
    git::{GitCliAdapter, GitWriteCommand, GitWriteCommandOutcome},
    manual_merge_resolution::{DigestEnvelopeFields, recompute_resolution_digest},
};

impl MergeContinuePort for GitCliAdapter {
    fn continue_merge(&mut self, request: &MergeContinueRequest) -> MergeContinueOutcome {
        match ContinueMerge::new(self, request).run() {
            Ok(outcome) => outcome,
            Err(()) => MergeContinueOutcome::PostWriteUncertain,
        }
    }
}

enum PendingClass {
    ReadyToContinue,
    Stale,
    Pending,
    Unclear,
}

struct ContinueMerge<'a> {
    git: &'a mut GitCliAdapter,
    request: &'a MergeContinueRequest,
}

impl<'a> ContinueMerge<'a> {
    const fn new(git: &'a mut GitCliAdapter, request: &'a MergeContinueRequest) -> Self {
        Self { git, request }
    }

    fn run(&mut self) -> Result<MergeContinueOutcome, ()> {
        match self.classify_pending()? {
            PendingClass::ReadyToContinue => {}
            PendingClass::Stale => return Ok(MergeContinueOutcome::ConfirmationStale),
            PendingClass::Pending => return Ok(MergeContinueOutcome::ConfirmedMergePending),
            PendingClass::Unclear => return Ok(MergeContinueOutcome::PreWriteRejected),
        }
        let root = self.root();
        let Some((name, email)) = self.git.commit_author_identity(&root).map_err(|_| ())? else {
            return Ok(MergeContinueOutcome::PreWriteRejected);
        };
        match self.run_continue(&name, &email) {
            GitWriteCommandOutcome::Succeeded => self.classify_after_success(),
            GitWriteCommandOutcome::Failed => self.classify_after_failure(),
            GitWriteCommandOutcome::TimedOut | GitWriteCommandOutcome::Uncertain => {
                Ok(MergeContinueOutcome::PostWriteUncertain)
            }
        }
    }

    fn root(&self) -> PathBuf {
        self.request.original_checkout.canonical_path.clone()
    }

    fn worktree(&self) -> PathBuf {
        self.request.task_worktree.canonical_path.clone()
    }

    fn envelope_fields(&self) -> DigestEnvelopeFields<'a> {
        DigestEnvelopeFields {
            task_id: self.request.task_id,
            project_id: self.request.project_id,
            merge_conflict_task_version: self.request.merge_conflict_task_version,
            source_approval_task_version: self.request.source_approval_task_version,
            base_branch: &self.request.base_branch,
            task_branch: &self.request.task_branch,
            base_commit: &self.request.base_commit,
        }
    }

    fn classify_pending(&mut self) -> Result<PendingClass, ()> {
        let candidate_request = ManualMergeResolutionCandidateRequest {
            original_checkout: self.request.original_checkout.clone(),
            original_common_dir: self.request.original_common_dir.clone(),
            task_worktree: self.request.task_worktree.clone(),
            task_id: self.request.task_id,
            project_id: self.request.project_id,
            merge_conflict_task_version: self.request.merge_conflict_task_version,
            source_approval_task_version: self.request.source_approval_task_version,
            task_branch: self.request.task_branch.clone(),
            base_branch: self.request.base_branch.clone(),
            base_commit: self.request.base_commit.clone(),
        };
        Ok(match self.git.resolution_candidate(&candidate_request) {
            ManualResolutionCandidateOutcome::Ready(candidate) => {
                if candidate.resolution_digest == self.request.confirmed_resolution_digest
                    && candidate.base_commit == self.request.base_commit
                    && candidate.task_commit == self.request.task_commit
                    && candidate.merge_head_commit == self.request.merge_head_commit
                {
                    PendingClass::ReadyToContinue
                } else {
                    PendingClass::Stale
                }
            }
            ManualResolutionCandidateOutcome::Unresolved => PendingClass::Pending,
            ManualResolutionCandidateOutcome::Inconsistent
            | ManualResolutionCandidateOutcome::Unavailable => PendingClass::Unclear,
        })
    }

    fn run_continue(&mut self, name: &str, email: &str) -> GitWriteCommandOutcome {
        let root = self.root();
        let name = OsStr::new(name);
        let email = OsStr::new(email);
        // `GIT_EDITOR=true` is the mechanism verified to prevent
        // `merge --continue`'s internal commit step from opening an
        // interactive editor for the already-prepared `MERGE_MSG`.
        // Confirmed empirically: with no editor override, Vim launches,
        // reads from this call's null stdin, and hangs indefinitely instead
        // of failing fast.
        //
        // `true` has no shell metacharacters, so Git executes the argv
        // directly with no shell wrapper; on Windows, Git then resolves it
        // via its own PATH lookup. This adapter's child process PATH is
        // restricted to the trusted Git installation's `cmd`/`bin` and
        // system directory only -- it never includes the task worktree,
        // the user's PATH, or the current directory -- so this fixed
        // literal always resolves to `mingw64\bin\true.exe` inside that
        // trusted Git-for-Windows installation.
        let editor = OsStr::new("true");
        self.git.run_write_command_with_env(
            &root,
            GitWriteCommand::Merge,
            ["merge", "--continue"],
            &[
                ("GIT_AUTHOR_NAME", name),
                ("GIT_AUTHOR_EMAIL", email),
                ("GIT_COMMITTER_NAME", name),
                ("GIT_COMMITTER_EMAIL", email),
                ("GIT_EDITOR", editor),
            ],
        )
    }

    fn head(&mut self, root: &std::path::Path) -> Option<String> {
        self.git
            .run_command(root, ["rev-parse", "--verify", "HEAD"])
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| GitCliAdapter::output_text(&output).ok().map(str::to_owned))
    }

    fn parents(&mut self, root: &std::path::Path, commit: &str) -> Option<Vec<String>> {
        let output = self
            .git
            .run_command(root, ["rev-list", "--parents", "-n", "1", commit])
            .ok()
            .filter(|output| output.status.success())?;
        let text = GitCliAdapter::output_text(&output).ok()?;
        Some(text.split_whitespace().map(str::to_owned).collect())
    }

    fn has_merge_residue(&self) -> bool {
        ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE", "MERGE_AUTOSTASH"]
            .iter()
            .any(|name| {
                self.request
                    .original_common_dir
                    .canonical_path
                    .join(name)
                    .exists()
            })
    }

    fn classify_after_success(&mut self) -> Result<MergeContinueOutcome, ()> {
        let root = self.root();
        let worktree = self.worktree();
        let Some(new_head) = self.head(&root) else {
            return Ok(MergeContinueOutcome::PostWriteUncertain);
        };
        let Some(parents) = self.parents(&root, &new_head) else {
            return Ok(MergeContinueOutcome::PostWriteUncertain);
        };
        let mut parts = parents.into_iter();
        let _merge_commit = parts.next();
        let expected_parents = (parts.next(), parts.next(), parts.next().is_none());
        if expected_parents
            != (
                Some(self.request.base_commit.clone()),
                Some(self.request.task_commit.clone()),
                true,
            )
        {
            return Ok(MergeContinueOutcome::PostWriteUncertain);
        }
        if self.has_merge_residue() {
            return Ok(MergeContinueOutcome::PostWriteUncertain);
        }
        let root_clean = self
            .git
            .repository_status(&root)
            .map(|status| {
                status.clean
                    && status.current_branch.as_deref() == Some(&self.request.base_branch)
                    && status.head_commit.as_deref() == Some(new_head.as_str())
            })
            .unwrap_or(false);
        let worktree_unchanged = self
            .git
            .repository_status(&worktree)
            .map(|status| {
                status.current_branch.as_deref() == Some(&self.request.task_branch)
                    && status.head_commit.as_deref() == Some(self.request.task_commit.as_str())
            })
            .unwrap_or(false);
        if !root_clean || !worktree_unchanged {
            return Ok(MergeContinueOutcome::PostWriteUncertain);
        }
        let fields = self.envelope_fields();
        let post_digest = recompute_resolution_digest(
            self.git,
            &root,
            &fields,
            &self.request.task_commit,
            &self.request.merge_head_commit,
        )?;
        if post_digest == Some(self.request.confirmed_resolution_digest) {
            Ok(MergeContinueOutcome::Continued)
        } else {
            Ok(MergeContinueOutcome::PostWriteUncertain)
        }
    }

    fn classify_after_failure(&mut self) -> Result<MergeContinueOutcome, ()> {
        let root = self.root();
        let head_unchanged = self.head(&root).as_deref() == Some(self.request.base_commit.as_str());
        let merge_head_still_expected = self
            .git
            .run_command(&root, ["rev-parse", "--verify", "MERGE_HEAD"])
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| GitCliAdapter::output_text(&output).ok().map(str::to_owned))
            .as_deref()
            == Some(self.request.merge_head_commit.as_str());
        if head_unchanged && merge_head_still_expected {
            Ok(MergeContinueOutcome::ConfirmedMergePending)
        } else {
            Ok(MergeContinueOutcome::PostWriteUncertain)
        }
    }
}
