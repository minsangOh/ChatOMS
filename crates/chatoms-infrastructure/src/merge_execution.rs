use std::path::PathBuf;

use chatoms_platform::supported_directory_identity;
use chatoms_ports::{
    diff::{CommitCandidateOutcome, CommitCandidatePort},
    filesystem::DirectoryIdentity,
    git::GitService,
    merge_execution::{
        MergeExecutionOutcome, MergeExecutionPort, MergeExecutionRequest, PreWriteRejection,
    },
};

use crate::git::{GitCliAdapter, GitWriteCommand, GitWriteCommandOutcome};

const TASK_COMMIT_MESSAGE: &str = "feat: apply approved task changes";
const MERGE_COMMIT_MESSAGE: &str = "merge: apply approved task changes";

impl MergeExecutionPort for GitCliAdapter {
    fn commit_and_merge(&mut self, request: &MergeExecutionRequest) -> MergeExecutionOutcome {
        match CommitAndMerge::new(self, request).run() {
            Ok(outcome) => outcome,
            Err(()) => MergeExecutionOutcome::PostWriteUncertain,
        }
    }
}

struct CommitAndMerge<'a> {
    git: &'a mut GitCliAdapter,
    request: &'a MergeExecutionRequest,
}

impl<'a> CommitAndMerge<'a> {
    const fn new(git: &'a mut GitCliAdapter, request: &'a MergeExecutionRequest) -> Self {
        Self { git, request }
    }

    fn run(&mut self) -> Result<MergeExecutionOutcome, ()> {
        if !self.identities_match() {
            return Ok(MergeExecutionOutcome::PreWriteRejected(
                PreWriteRejection::IdentityOrTopology,
            ));
        }
        if self.has_merge_residue() {
            return Ok(MergeExecutionOutcome::PreWriteRejected(
                PreWriteRejection::ExistingMergeResidue,
            ));
        }
        if !self.original_checkout_is_ready() {
            return Ok(MergeExecutionOutcome::PreWriteRejected(
                PreWriteRejection::OriginalCheckoutNotReady,
            ));
        }
        if !self.safe_repository_configuration() {
            return Ok(MergeExecutionOutcome::PreWriteRejected(
                PreWriteRejection::UnsafeRepositoryConfiguration,
            ));
        }
        if !self.has_author() {
            return Ok(MergeExecutionOutcome::PreWriteRejected(
                PreWriteRejection::AuthorUnavailable,
            ));
        }
        if !self.approved_candidate_matches() {
            return Ok(MergeExecutionOutcome::PreWriteRejected(
                PreWriteRejection::ApprovedCandidateMismatch,
            ));
        }

        match self.stage_candidate() {
            GitWriteCommandOutcome::Succeeded => {}
            GitWriteCommandOutcome::Failed
            | GitWriteCommandOutcome::TimedOut
            | GitWriteCommandOutcome::Uncertain => {
                return Ok(MergeExecutionOutcome::StageWriteUncertain);
            }
        }
        match self.create_task_commit() {
            GitWriteCommandOutcome::Succeeded => {}
            GitWriteCommandOutcome::Failed => return Ok(MergeExecutionOutcome::CommitNotCreated),
            GitWriteCommandOutcome::TimedOut | GitWriteCommandOutcome::Uncertain => {
                return Ok(MergeExecutionOutcome::PostWriteUncertain);
            }
        }
        let task_commit = self.task_head().ok_or(())?;
        if !self.task_commit_is_ready(&task_commit) {
            return Ok(MergeExecutionOutcome::PostWriteUncertain);
        }
        match self.merge_task_commit(&task_commit) {
            GitWriteCommandOutcome::Succeeded => {}
            GitWriteCommandOutcome::Failed => return Ok(self.classify_merge_failure(&task_commit)),
            GitWriteCommandOutcome::TimedOut | GitWriteCommandOutcome::Uncertain => {
                return Ok(MergeExecutionOutcome::PostWriteUncertain);
            }
        }
        if self.merged_task_commit(&task_commit) {
            Ok(MergeExecutionOutcome::Merged)
        } else {
            Ok(MergeExecutionOutcome::PostWriteUncertain)
        }
    }

    fn root(&self) -> PathBuf {
        self.request.original_checkout.canonical_path.clone()
    }

    fn worktree(&self) -> PathBuf {
        self.request.task_worktree.canonical_path.clone()
    }

    fn identities_match(&mut self) -> bool {
        matches_identity(&self.request.original_checkout)
            && matches_identity(&self.request.original_common_dir)
            && matches_identity(&self.request.task_worktree)
            && self.request.original_common_dir.canonical_path == self.root().join(".git")
            && {
                let root = self.root();
                let worktree = self.worktree();
                self.git
                    .verify_task_worktree_with_changes(
                        &root,
                        &self.request.task_branch,
                        &self.request.base_commit,
                        &worktree,
                    )
                    .unwrap_or(false)
            }
    }

    /// `MERGE_AUTOSTASH` is listed alongside the other three: it is merge
    /// residue exactly like them, and `crate::merge_continue`,
    /// `crate::merge_abort` and `crate::merge_conflict_inspection` all
    /// already treat it as such. Leaving it out here meant a checkout
    /// carrying a leftover autostash entry was not rejected as
    /// `ExistingMergeResidue`, so this adapter would go on to stage, commit
    /// and merge on top of it.
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

    fn original_checkout_is_ready(&mut self) -> bool {
        let root = self.root();
        self.git
            .repository_status(&root)
            .map(|status| {
                status.clean
                    && status.current_branch.as_deref() == Some(&self.request.base_branch)
                    && status.head_commit.as_deref() == Some(&self.request.base_commit)
            })
            .unwrap_or(false)
    }

    fn safe_repository_configuration(&mut self) -> bool {
        let root = self.root();
        let worktree = self.worktree();
        self.git
            .validate_write_configuration(&root, &worktree, &self.request.base_commit)
            .is_ok()
    }

    fn has_author(&mut self) -> bool {
        let root = self.root();
        self.git.has_commit_author(&root).unwrap_or(false)
    }

    fn approved_candidate_matches(&mut self) -> bool {
        let root = self.root();
        let worktree = self.worktree();
        matches!(
            self.git.current_commit_candidate(
                &root,
                &self.request.base_branch,
                &self.request.task_branch,
                &self.request.base_commit,
                &worktree,
            ),
            Ok(CommitCandidateOutcome::Candidate(candidate))
                if candidate.content_hash() == self.request.approved_diff_content_hash
        )
    }

    fn stage_candidate(&mut self) -> GitWriteCommandOutcome {
        let worktree = self.worktree();
        self.git
            .run_write_command(&worktree, GitWriteCommand::Stage, ["add", "-A", "--", "."])
    }

    fn create_task_commit(&mut self) -> GitWriteCommandOutcome {
        let worktree = self.worktree();
        self.git.run_write_command(
            &worktree,
            GitWriteCommand::Commit,
            ["commit", "--no-gpg-sign", "-m", TASK_COMMIT_MESSAGE],
        )
    }

    fn task_head(&mut self) -> Option<String> {
        let worktree = self.worktree();
        self.git
            .run_command(&worktree, ["rev-parse", "--verify", "HEAD"])
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| GitCliAdapter::output_text(&output).ok().map(str::to_owned))
    }

    fn merge_task_commit(&mut self, task_commit: &str) -> GitWriteCommandOutcome {
        let root = self.root();
        self.git.run_write_command(
            &root,
            GitWriteCommand::Merge,
            [
                "merge",
                "--no-ff",
                "--no-gpg-sign",
                "-m",
                MERGE_COMMIT_MESSAGE,
                task_commit,
            ],
        )
    }

    fn task_commit_is_ready(&mut self, task_commit: &str) -> bool {
        let worktree = self.worktree();
        let output = match self
            .git
            .run_command(&worktree, ["rev-list", "--parents", "-n", "1", "HEAD"])
        {
            Ok(output) if output.status.success() => output,
            _ => return false,
        };
        let Ok(parents) = GitCliAdapter::output_text(&output) else {
            return false;
        };
        let mut parts = parents.split_whitespace();
        parts.next() == Some(task_commit)
            && parts.next() == Some(self.request.base_commit.as_str())
            && parts.next().is_none()
            && self.git.repository_status(&worktree).is_ok_and(|status| {
                status.clean
                    && status.current_branch.as_deref() == Some(&self.request.task_branch)
                    && status.head_commit.as_deref() == Some(task_commit)
            })
    }

    fn classify_merge_failure(&mut self, task_commit: &str) -> MergeExecutionOutcome {
        if self.has_merge_residue() {
            return MergeExecutionOutcome::ConfirmedMergeConflict;
        }
        let root = self.root();
        let root_unchanged = self
            .git
            .repository_status(&root)
            .map(|status| {
                status.clean && status.head_commit.as_deref() == Some(&self.request.base_commit)
            })
            .unwrap_or(false);
        let task_commit_still_exists = self.task_head().as_deref() == Some(task_commit);
        if root_unchanged && task_commit_still_exists {
            MergeExecutionOutcome::CommitSucceededMergeFailed
        } else {
            MergeExecutionOutcome::PostWriteUncertain
        }
    }

    /// Postcondition for a merge this adapter believes succeeded, at the
    /// same strength as `crate::merge_continue`'s `classify_after_success`.
    ///
    /// Every clause must hold: the original checkout is on the expected base
    /// branch; `HEAD` is the merge commit this call just observed; that
    /// commit has exactly two parents in the order (approved base commit,
    /// task commit); no merge residue is left behind; and the checkout is
    /// clean. Anything short of that returns `false`, and the caller maps
    /// that to the existing `PostWriteUncertain` outcome — a merge is never
    /// reported as `Merged` on the strength of the parent list alone.
    ///
    /// No commit hash, path, or Git output read here reaches a DTO, the UI,
    /// or an error: the only thing that escapes is this boolean.
    fn merged_task_commit(&mut self, task_commit: &str) -> bool {
        let root = self.root();
        let Some(new_head) = self.root_head() else {
            return false;
        };
        let output = match self
            .git
            .run_command(&root, ["rev-list", "--parents", "-n", "1", &new_head])
        {
            Ok(output) if output.status.success() => output,
            _ => return false,
        };
        let Ok(parents) = GitCliAdapter::output_text(&output) else {
            return false;
        };
        let mut parts = parents.split_whitespace();
        if parts.next() != Some(new_head.as_str())
            || parts.next() != Some(self.request.base_commit.as_str())
            || parts.next() != Some(task_commit)
            || parts.next().is_some()
        {
            return false;
        }
        if self.has_merge_residue() {
            return false;
        }
        self.git.repository_status(&root).is_ok_and(|status| {
            status.clean
                && status.current_branch.as_deref() == Some(&self.request.base_branch)
                && status.head_commit.as_deref() == Some(new_head.as_str())
        })
    }

    /// The original checkout's current `HEAD`, mirroring [`Self::task_head`]
    /// for the task worktree.
    fn root_head(&mut self) -> Option<String> {
        let root = self.root();
        self.git
            .run_command(&root, ["rev-parse", "--verify", "HEAD"])
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| GitCliAdapter::output_text(&output).ok().map(str::to_owned))
    }
}

fn matches_identity(expected: &DirectoryIdentity) -> bool {
    supported_directory_identity(&expected.canonical_path).is_ok_and(|actual| {
        actual.same_object(expected) && actual.canonical_path == expected.canonical_path
    })
}
