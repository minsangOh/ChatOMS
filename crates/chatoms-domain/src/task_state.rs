use serde::{Deserialize, Serialize};

use crate::DomainError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum TaskState {
    Created,
    ProjectValidated,
    AwaitingGitInitApproval,
    GitInitialized,
    WorktreeCreating,
    WorktreeReady,
    PlanningWithClaude,
    AwaitingDesignApproval,
    ImplementingWithCodex,
    Testing,
    AutoFixing,
    ReviewingWithClaude,
    ReviewFixing,
    AwaitingUserDiffApproval,
    Merging,
    MergeConflict,
    PostMergeTesting,
    Completed,
    Paused,
    Failed,
    RecoveryRequired,
    UnknownExternalEffect,
    Cancelled,
    CleanupPending,
    Archived,
}

impl TaskState {
    pub const ALL: [Self; 25] = [
        Self::Created,
        Self::ProjectValidated,
        Self::AwaitingGitInitApproval,
        Self::GitInitialized,
        Self::WorktreeCreating,
        Self::WorktreeReady,
        Self::PlanningWithClaude,
        Self::AwaitingDesignApproval,
        Self::ImplementingWithCodex,
        Self::Testing,
        Self::AutoFixing,
        Self::ReviewingWithClaude,
        Self::ReviewFixing,
        Self::AwaitingUserDiffApproval,
        Self::Merging,
        Self::MergeConflict,
        Self::PostMergeTesting,
        Self::Completed,
        Self::Paused,
        Self::Failed,
        Self::RecoveryRequired,
        Self::UnknownExternalEffect,
        Self::Cancelled,
        Self::CleanupPending,
        Self::Archived,
    ];

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    #[must_use]
    pub const fn is_post_terminal(self) -> bool {
        matches!(self, Self::CleanupPending | Self::Archived)
    }

    #[must_use]
    pub const fn requires_active_lease(self) -> bool {
        !self.is_terminal() && !self.is_post_terminal()
    }

    #[must_use]
    pub const fn is_recoverable(self) -> bool {
        self.requires_active_lease()
    }

    #[must_use]
    pub const fn allows_cleanup(self) -> bool {
        matches!(self, Self::CleanupPending)
    }

    #[must_use]
    pub const fn is_resume_target(self) -> bool {
        !matches!(
            self,
            Self::Paused
                | Self::RecoveryRequired
                | Self::UnknownExternalEffect
                | Self::Completed
                | Self::Failed
                | Self::Cancelled
                | Self::CleanupPending
                | Self::Archived
        )
    }

    #[must_use]
    pub const fn can_pause(self) -> bool {
        matches!(
            self,
            Self::AwaitingGitInitApproval
                | Self::WorktreeReady
                | Self::PlanningWithClaude
                | Self::AwaitingDesignApproval
                | Self::ImplementingWithCodex
                | Self::Testing
                | Self::AutoFixing
                | Self::ReviewingWithClaude
                | Self::ReviewFixing
                | Self::AwaitingUserDiffApproval
                | Self::MergeConflict
        )
    }

    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        use TaskState::{
            Archived, AutoFixing, AwaitingDesignApproval, AwaitingGitInitApproval,
            AwaitingUserDiffApproval, Cancelled, CleanupPending, Completed, Created, Failed,
            GitInitialized, ImplementingWithCodex, MergeConflict, Merging, PlanningWithClaude,
            PostMergeTesting, ProjectValidated, RecoveryRequired, ReviewFixing,
            ReviewingWithClaude, Testing, UnknownExternalEffect, WorktreeCreating, WorktreeReady,
        };

        matches!(
            (self, next),
            (
                Created,
                ProjectValidated | AwaitingGitInitApproval | Cancelled | Failed
            ) | (ProjectValidated, WorktreeCreating | Cancelled | Failed)
                | (AwaitingGitInitApproval, GitInitialized | Cancelled)
                | (GitInitialized, WorktreeCreating | Failed)
                | (
                    WorktreeCreating,
                    WorktreeReady | RecoveryRequired | Failed | Cancelled
                )
                | (WorktreeReady, PlanningWithClaude | Cancelled)
                | (
                    PlanningWithClaude,
                    AwaitingDesignApproval | ImplementingWithCodex | Failed | RecoveryRequired
                )
                | (AwaitingDesignApproval, ImplementingWithCodex | Cancelled)
                | (ImplementingWithCodex, Testing | Failed | RecoveryRequired)
                | (
                    Testing,
                    AutoFixing | ReviewingWithClaude | Failed | RecoveryRequired
                )
                | (AutoFixing, Testing | Failed | RecoveryRequired)
                | (
                    ReviewingWithClaude,
                    ReviewFixing | AwaitingUserDiffApproval | Failed | RecoveryRequired
                )
                | (ReviewFixing, Testing | Failed | RecoveryRequired)
                | (AwaitingUserDiffApproval, Merging | Cancelled)
                | (
                    Merging,
                    PostMergeTesting | MergeConflict | RecoveryRequired | Failed
                )
                | (MergeConflict, Merging | Cancelled | Failed)
                | (PostMergeTesting, Completed | Failed | RecoveryRequired)
                | (Completed, CleanupPending | Archived)
                | (TaskState::Paused, RecoveryRequired | Cancelled | Failed)
                | (Failed, CleanupPending | Archived)
                | (RecoveryRequired, Cancelled | Failed)
                | (UnknownExternalEffect, RecoveryRequired | Cancelled | Failed)
                | (Cancelled, CleanupPending | Archived)
                | (CleanupPending, Archived)
        )
    }

    #[must_use]
    pub const fn can_contextually_transition_to(self, next: Self) -> bool {
        (self.can_pause() && matches!(next, Self::Paused))
            || (matches!(self, Self::Paused) && next.is_resume_target())
            || (matches!(self, Self::RecoveryRequired)
                && (matches!(next, Self::Paused) || next.is_resume_target()))
    }

    pub fn validate_transition(self, next: Self) -> Result<(), DomainError> {
        if self.can_transition_to(next) {
            Ok(())
        } else {
            Err(DomainError::InvalidStateTransition)
        }
    }
}
