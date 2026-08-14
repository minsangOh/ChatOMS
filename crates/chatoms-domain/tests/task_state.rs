mod support;

use std::collections::HashSet;

use chatoms_domain::{DomainError, TaskState};
use serde::{Deserialize, Serialize, de::value};

use support::StringSerializer;

#[test]
fn state_list_has_exactly_25_unique_pascal_case_values() {
    assert_eq!(TaskState::ALL.len(), 25);
    assert_eq!(TaskState::ALL.into_iter().collect::<HashSet<_>>().len(), 25);

    for state in TaskState::ALL {
        let serialized = state
            .serialize(StringSerializer)
            .expect("unit enum must serialize as its PascalCase variant");
        let deserializer = value::StrDeserializer::<value::Error>::new(&serialized);
        let deserialized =
            TaskState::deserialize(deserializer).expect("serialized state must deserialize");
        assert_eq!(deserialized, state);
    }

    let invalid = TaskState::deserialize(value::StrDeserializer::<value::Error>::new("NotAState"));
    assert!(invalid.is_err());

    for (state, serialized) in [
        (TaskState::Planning, "Planning"),
        (TaskState::Implementing, "Implementing"),
        (TaskState::Reviewing, "Reviewing"),
    ] {
        assert_eq!(
            state
                .serialize(StringSerializer)
                .expect("serialize renamed state"),
            serialized
        );
    }
    for legacy in [
        "PlanningWithClaude",
        "ImplementingWithCodex",
        "ReviewingWithClaude",
    ] {
        assert!(
            TaskState::deserialize(value::StrDeserializer::<value::Error>::new(legacy)).is_err(),
            "legacy provider-bound state must not deserialize: {legacy}"
        );
    }
}

#[test]
fn state_classification_matches_lease_and_cleanup_policy() {
    let terminal = [
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ];
    let post_terminal = [TaskState::CleanupPending, TaskState::Archived];

    for state in TaskState::ALL {
        assert_eq!(state.is_terminal(), terminal.contains(&state));
        assert_eq!(state.is_post_terminal(), post_terminal.contains(&state));
        assert_eq!(
            state.requires_active_lease(),
            !terminal.contains(&state) && !post_terminal.contains(&state)
        );
        assert_eq!(state.is_recoverable(), state.requires_active_lease());
        assert_eq!(state.allows_cleanup(), state == TaskState::CleanupPending);
    }

    for state in [
        TaskState::Paused,
        TaskState::RecoveryRequired,
        TaskState::UnknownExternalEffect,
    ] {
        assert!(state.requires_active_lease());
    }
}

#[test]
fn complete_static_transition_matrix_matches_independent_expectation() {
    use TaskState::*;

    let expected = HashSet::from([
        (Created, ProjectValidated),
        (Created, AwaitingGitInitApproval),
        (Created, Cancelled),
        (Created, Failed),
        (ProjectValidated, WorktreeCreating),
        (ProjectValidated, Cancelled),
        (ProjectValidated, Failed),
        (AwaitingGitInitApproval, GitInitialized),
        (AwaitingGitInitApproval, RecoveryRequired),
        (AwaitingGitInitApproval, Cancelled),
        (GitInitialized, WorktreeCreating),
        (GitInitialized, Failed),
        (WorktreeCreating, WorktreeReady),
        (WorktreeCreating, RecoveryRequired),
        (WorktreeCreating, Failed),
        (WorktreeCreating, Cancelled),
        (WorktreeReady, Planning),
        (WorktreeReady, Cancelled),
        (Planning, AwaitingDesignApproval),
        (Planning, Implementing),
        (Planning, Failed),
        (Planning, RecoveryRequired),
        (Planning, Cancelled),
        (AwaitingDesignApproval, Implementing),
        (AwaitingDesignApproval, Cancelled),
        (Implementing, Testing),
        (Implementing, Failed),
        (Implementing, RecoveryRequired),
        (Testing, AutoFixing),
        (Testing, Reviewing),
        (Testing, Failed),
        (Testing, RecoveryRequired),
        (AutoFixing, Testing),
        (AutoFixing, Failed),
        (AutoFixing, RecoveryRequired),
        (Reviewing, ReviewFixing),
        (Reviewing, AwaitingUserDiffApproval),
        (Reviewing, Failed),
        (Reviewing, RecoveryRequired),
        (ReviewFixing, Testing),
        (ReviewFixing, Failed),
        (ReviewFixing, RecoveryRequired),
        (AwaitingUserDiffApproval, Merging),
        (AwaitingUserDiffApproval, Cancelled),
        (Merging, PostMergeTesting),
        (Merging, MergeConflict),
        (Merging, RecoveryRequired),
        (Merging, Failed),
        (MergeConflict, Merging),
        (MergeConflict, Cancelled),
        (MergeConflict, Failed),
        (PostMergeTesting, Completed),
        (PostMergeTesting, Failed),
        (PostMergeTesting, RecoveryRequired),
        (Completed, CleanupPending),
        (Completed, Archived),
        (Paused, RecoveryRequired),
        (Paused, Cancelled),
        (Paused, Failed),
        (Failed, CleanupPending),
        (Failed, Archived),
        (RecoveryRequired, Cancelled),
        (RecoveryRequired, Failed),
        (UnknownExternalEffect, RecoveryRequired),
        (UnknownExternalEffect, Cancelled),
        (UnknownExternalEffect, Failed),
        (Cancelled, CleanupPending),
        (Cancelled, Archived),
        (CleanupPending, Archived),
    ]);

    for current in TaskState::ALL {
        for next in TaskState::ALL {
            let should_allow = expected.contains(&(current, next));
            assert_eq!(
                current.can_transition_to(next),
                should_allow,
                "unexpected static transition result: {current:?} -> {next:?}"
            );
            assert_eq!(
                current.validate_transition(next),
                should_allow
                    .then_some(())
                    .ok_or(DomainError::InvalidStateTransition)
            );
            if current == next {
                assert!(!current.can_transition_to(next));
            }
        }
    }
    assert!(
        TaskState::ALL
            .into_iter()
            .all(|next| !Archived.can_transition_to(next))
    );
}

#[test]
fn contextual_and_static_transition_categories_do_not_overlap() {
    use TaskState::*;

    let pause_sources = [
        AwaitingGitInitApproval,
        WorktreeReady,
        Planning,
        AwaitingDesignApproval,
        Implementing,
        Testing,
        AutoFixing,
        Reviewing,
        ReviewFixing,
        AwaitingUserDiffApproval,
        MergeConflict,
    ];
    let resume_targets = [
        Created,
        ProjectValidated,
        AwaitingGitInitApproval,
        GitInitialized,
        WorktreeCreating,
        WorktreeReady,
        Planning,
        AwaitingDesignApproval,
        Implementing,
        Testing,
        AutoFixing,
        Reviewing,
        ReviewFixing,
        AwaitingUserDiffApproval,
        Merging,
        MergeConflict,
        PostMergeTesting,
    ];
    let mut contextual = HashSet::new();
    contextual.extend(pause_sources.map(|source| (source, Paused)));
    contextual.extend(resume_targets.map(|target| (Paused, target)));
    contextual.extend(resume_targets.map(|target| (RecoveryRequired, target)));
    contextual.insert((RecoveryRequired, Paused));

    for current in TaskState::ALL {
        for next in TaskState::ALL {
            let is_static = current.can_transition_to(next);
            let is_contextual = current.can_contextually_transition_to(next);
            assert_eq!(is_contextual, contextual.contains(&(current, next)));
            assert!(!(is_static && is_contextual));
        }
    }

    assert!(!UnknownExternalEffect.can_transition_to(Paused));
    assert!(UnknownExternalEffect.can_transition_to(RecoveryRequired));
    assert!(!RecoveryRequired.can_transition_to(Paused));
    assert!(RecoveryRequired.can_contextually_transition_to(Paused));
}
