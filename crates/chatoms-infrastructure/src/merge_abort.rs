//! `git merge --abort` on the original checkout of a task's `MergeConflict`
//! that a user has explicitly approved to abandon. This is the only Git
//! write this adapter performs: no `--quit`, `reset`, `checkout`,
//! `restore`, `stash`, or `clean`.
//!
//! Unlike `crate::merge_continue`, this write does not require a `Ready`
//! manual-resolution candidate -- the primary use case is aborting while
//! conflicts are still unresolved. Identity/topology and foreign operation
//! residue are checked directly by this module (see [`AbortMerge::run`]'s
//! documentation for why), and [`MergeConflictInspectionPort::inspect_merge_conflicts`]
//! is reused only to classify whether a genuine mid-merge state still
//! exists for this task/base pair. The restoration postcondition this
//! module checks is shared between three call sites: classifying a
//! not-currently-merging repository before any write
//! (`ConfirmedNotInMerge` vs. `NotInMergeAndNotRestored`), and classifying
//! both `merge --abort` write outcomes (`Aborted`/`ConfirmedNotInMerge` vs.
//! `PostWriteUncertain`) -- a prior successful abort whose SQLite commit
//! never landed must be recognized as already-restored on the next attempt
//! rather than retried or rejected.

use std::path::PathBuf;

use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    git::GitService,
    merge_abort::{
        MergeAbortOutcome, MergeAbortPort, MergeAbortPreWriteRejection, MergeAbortRequest,
    },
    merge_conflict_inspection::{
        MergeConflictInspectionOutcome, MergeConflictInspectionPort, MergeConflictInspectionRequest,
    },
};

use crate::git::{GitCliAdapter, GitWriteCommand, GitWriteCommandOutcome};

impl MergeAbortPort for GitCliAdapter {
    fn abort_merge(&mut self, request: &MergeAbortRequest) -> MergeAbortOutcome {
        match AbortMerge::new(self, request).run() {
            Ok(outcome) => outcome,
            Err(()) => MergeAbortOutcome::PostWriteUncertain,
        }
    }
}

struct AbortMerge<'a> {
    git: &'a mut GitCliAdapter,
    request: &'a MergeAbortRequest,
}

impl<'a> AbortMerge<'a> {
    const fn new(git: &'a mut GitCliAdapter, request: &'a MergeAbortRequest) -> Self {
        Self { git, request }
    }

    /// Checks identity/topology and foreign operation residue itself,
    /// *before* calling [`MergeConflictInspectionPort::inspect_merge_conflicts`]
    /// -- that port bundles both checks (plus branch/commit topology and
    /// live `MERGE_HEAD`/task-commit agreement) into a single `Inconsistent`
    /// outcome without saying which failed, so relying on it alone would
    /// make [`MergeAbortPreWriteRejection::IdentityOrTopology`] and
    /// [`MergeAbortPreWriteRejection::ForeignOperationResidue`] impossible
    /// to distinguish from "not currently in a merge at all". Checking them
    /// here first, with this module's own independent helpers, keeps both
    /// reasons precise and never depends on `inspect_merge_conflicts`'s
    /// classification to detect them.
    fn run(&mut self) -> Result<MergeAbortOutcome, ()> {
        if !self.identity_matches()? {
            return Ok(MergeAbortOutcome::PreWriteRejected(
                MergeAbortPreWriteRejection::IdentityOrTopology,
            ));
        }
        if self.has_foreign_operation_residue()? {
            return Ok(MergeAbortOutcome::PreWriteRejected(
                MergeAbortPreWriteRejection::ForeignOperationResidue,
            ));
        }
        let inspection = self
            .git
            .inspect_merge_conflicts(&MergeConflictInspectionRequest {
                original_checkout: self.request.original_checkout.clone(),
                original_common_dir: self.request.original_common_dir.clone(),
                task_worktree: self.request.task_worktree.clone(),
                task_branch: self.request.task_branch.clone(),
                base_branch: self.request.base_branch.clone(),
                base_commit: self.request.base_commit.clone(),
            });
        match inspection.outcome {
            MergeConflictInspectionOutcome::ConfirmedUnresolved
            | MergeConflictInspectionOutcome::ResolvedPendingConfirmation => {
                self.abort_confirmed_merge()
            }
            // `RestoredPendingAbortConfirmation` is folded into the same
            // branch as `Inconsistent`/`Unavailable`, not treated as
            // sufficient proof on its own: that outcome only confirms the
            // *general* shape of a restored repository (structural
            // topology, not this specific approval's exact
            // `task_commit`/`merge_head_commit`), so the write-path
            // decision here still always re-runs this module's own
            // independent, strictly-scoped `restoration_holds` check rather
            // than trusting the inspection port's classification.
            MergeConflictInspectionOutcome::Inconsistent
            | MergeConflictInspectionOutcome::Unavailable
            | MergeConflictInspectionOutcome::RestoredPendingAbortConfirmation => {
                if self.restoration_holds()? {
                    Ok(MergeAbortOutcome::ConfirmedNotInMerge)
                } else {
                    Ok(MergeAbortOutcome::PreWriteRejected(
                        MergeAbortPreWriteRejection::NotInMergeAndNotRestored,
                    ))
                }
            }
        }
    }

    /// Reached only when identity/topology and foreign operation residue
    /// are already confirmed clean, and `inspect_merge_conflicts` has
    /// confirmed a genuine mid-merge state for this task/base pair -- but
    /// it only checks that the *live* `MERGE_HEAD` equals the *live* task
    /// worktree `HEAD`, never that either matches this request's approved
    /// `task_commit`/`merge_head_commit`. That comparison happens here,
    /// since it is what actually detects a stale approval (e.g. a new
    /// commit landed on the task branch after approval, changing the live
    /// task commit).
    fn abort_confirmed_merge(&mut self) -> Result<MergeAbortOutcome, ()> {
        let worktree = self.worktree();
        let Some(live_task_commit) = self.head(&worktree) else {
            return Ok(MergeAbortOutcome::PreWriteRejected(
                MergeAbortPreWriteRejection::IdentityOrTopology,
            ));
        };
        if live_task_commit != self.request.task_commit
            || live_task_commit != self.request.merge_head_commit
        {
            return Ok(MergeAbortOutcome::PreWriteRejected(
                MergeAbortPreWriteRejection::MergeIdentityMismatch,
            ));
        }
        if self.has_autostash() {
            return Ok(MergeAbortOutcome::PreWriteRejected(
                MergeAbortPreWriteRejection::AutostashPresent,
            ));
        }
        let root = self.root();
        if self
            .git
            .validate_write_configuration(&root, &worktree, &self.request.base_commit)
            .is_err()
        {
            return Ok(MergeAbortOutcome::PreWriteRejected(
                MergeAbortPreWriteRejection::UnsafeRepositoryConfiguration,
            ));
        }
        match self
            .git
            .run_write_command(&root, GitWriteCommand::MergeAbort, ["merge", "--abort"])
        {
            GitWriteCommandOutcome::Succeeded => {
                if self.restoration_holds()? {
                    Ok(MergeAbortOutcome::Aborted)
                } else {
                    Ok(MergeAbortOutcome::PostWriteUncertain)
                }
            }
            GitWriteCommandOutcome::Failed => {
                if self.restoration_holds()? {
                    Ok(MergeAbortOutcome::ConfirmedNotInMerge)
                } else {
                    Ok(MergeAbortOutcome::PostWriteUncertain)
                }
            }
            GitWriteCommandOutcome::TimedOut | GitWriteCommandOutcome::Uncertain => {
                Ok(MergeAbortOutcome::PostWriteUncertain)
            }
        }
    }

    fn root(&self) -> PathBuf {
        self.request.original_checkout.canonical_path.clone()
    }

    fn worktree(&self) -> PathBuf {
        self.request.task_worktree.canonical_path.clone()
    }

    fn head(&mut self, root: &std::path::Path) -> Option<String> {
        self.git
            .run_command(root, ["rev-parse", "--verify", "HEAD"])
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| GitCliAdapter::output_text(&output).ok().map(str::to_owned))
    }

    fn has_autostash(&self) -> bool {
        self.request
            .original_common_dir
            .canonical_path
            .join("MERGE_AUTOSTASH")
            .exists()
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

    fn has_foreign_operation_residue(&self) -> Result<bool, ()> {
        for name in [
            "REBASE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "BISECT_START",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
        ] {
            match std::fs::symlink_metadata(
                self.request.original_common_dir.canonical_path.join(name),
            ) {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(()),
            }
        }
        Ok(false)
    }

    fn identity_matches(&self) -> Result<bool, ()> {
        for expected in [
            &self.request.original_checkout,
            &self.request.original_common_dir,
            &self.request.task_worktree,
        ] {
            let actual = supported_directory_identity(&expected.canonical_path).map_err(|_| ())?;
            if actual.canonical_path != expected.canonical_path || !actual.same_object(expected) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Independent restoration postcondition, reused by every branch that
    /// needs to confirm the repository is fully back to its pre-merge
    /// state: no merge in progress, no residue of any kind (including
    /// foreign rebase/cherry-pick/revert/bisect/sequencer operations), the
    /// original checkout back on `base_branch` at exactly `base_commit` and
    /// clean, and the task worktree unaffected (still on `task_branch` at
    /// exactly `task_commit`).
    fn restoration_holds(&mut self) -> Result<bool, ()> {
        if !self.identity_matches()? {
            return Ok(false);
        }
        if self.has_merge_residue() || self.has_foreign_operation_residue()? {
            return Ok(false);
        }
        let root = self.root();
        let worktree = self.worktree();
        let root_restored = self
            .git
            .repository_status(&root)
            .map(|status| {
                status.clean
                    && status.current_branch.as_deref() == Some(self.request.base_branch.as_str())
                    && status.head_commit.as_deref() == Some(self.request.base_commit.as_str())
            })
            .unwrap_or(false);
        if !root_restored {
            return Ok(false);
        }
        let worktree_unchanged = self
            .git
            .repository_status(&worktree)
            .map(|status| {
                status.current_branch.as_deref() == Some(self.request.task_branch.as_str())
                    && status.head_commit.as_deref() == Some(self.request.task_commit.as_str())
            })
            .unwrap_or(false);
        Ok(worktree_unchanged)
    }
}
