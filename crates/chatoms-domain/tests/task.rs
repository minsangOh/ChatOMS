use std::str::FromStr;

use chatoms_domain::{
    ACTOR_KIND_MAX_LENGTH, ActorKind, DomainError, ProjectId, REASON_CODE_MAX_LENGTH, ReasonCode,
    RecoveryValidation, ResumeValidation, Task, TaskBranchIdentity, TaskId, TaskState,
    TaskStateTransition, TaskStateTransitionId, TaskStateTransitionSnapshot,
};

fn new_task(created_at_ms: i64) -> Task {
    Task::new(TaskId::new(), ProjectId::new(), created_at_ms)
}

fn worktree_ready_task() -> Task {
    let mut task = new_task(100);
    task.transition_to(TaskState::ProjectValidated, 101)
        .expect("Created -> ProjectValidated");
    task.transition_to(TaskState::WorktreeCreating, 102)
        .expect("ProjectValidated -> WorktreeCreating");
    task.transition_to(TaskState::WorktreeReady, 103)
        .expect("WorktreeCreating -> WorktreeReady");
    task
}

fn recovery_required_task() -> Task {
    let mut task = new_task(100);
    task.transition_to(TaskState::ProjectValidated, 101)
        .expect("Created -> ProjectValidated");
    task.transition_to(TaskState::WorktreeCreating, 102)
        .expect("ProjectValidated -> WorktreeCreating");
    task.transition_to(TaskState::RecoveryRequired, 103)
        .expect("WorktreeCreating -> RecoveryRequired");
    task
}

#[test]
fn task_creation_sets_identity_and_immutable_aggregate_fields() {
    let task_id = TaskId::new();
    let project_id = ProjectId::new();
    let task = Task::new(task_id, project_id, -10);

    assert_eq!(task.id(), task_id);
    assert_eq!(task.project_id(), project_id);
    assert_eq!(task.state(), TaskState::Created);
    assert_eq!(task.version(), 0);
    assert_eq!(
        task.task_branch_identity(),
        &TaskBranchIdentity::for_task(task_id)
    );
    assert_eq!(task.created_at_ms(), -10);
    assert_eq!(task.updated_at_ms(), -10);
    assert_eq!(task.terminal_at_ms(), None);
    assert_eq!(task.resume_target_state(), None);
    assert_eq!(task.validate_invariants(), Ok(()));
}

#[test]
fn static_transition_updates_version_and_rejects_invalid_change_atomically() {
    let mut task = new_task(100);
    let project_id = task.project_id();
    let branch = task.task_branch_identity().clone();
    task.transition_to(TaskState::ProjectValidated, 101)
        .expect("valid transition");
    assert_eq!(task.state(), TaskState::ProjectValidated);
    assert_eq!(task.version(), 1);
    assert_eq!(task.updated_at_ms(), 101);
    assert_eq!(task.project_id(), project_id);
    assert_eq!(task.task_branch_identity(), &branch);

    let before = task.clone();
    assert_eq!(
        task.transition_to(TaskState::Completed, 102),
        Err(DomainError::InvalidStateTransition)
    );
    assert_eq!(task, before);
}

#[test]
fn timestamp_regression_is_rejected_without_mutation() {
    let mut task = new_task(100);
    let before = task.clone();
    assert_eq!(
        task.transition_to(TaskState::ProjectValidated, 99),
        Err(DomainError::InvalidTimestamp)
    );
    assert_eq!(task, before);
}

#[test]
fn terminal_transition_sets_and_post_terminal_transition_retains_timestamp() {
    let mut task = new_task(100);
    task.transition_to(TaskState::Failed, 110)
        .expect("Created -> Failed");
    assert_eq!(task.terminal_at_ms(), Some(110));
    assert!(!task.state().requires_active_lease());

    task.transition_to(TaskState::CleanupPending, 120)
        .expect("Failed -> CleanupPending");
    assert_eq!(task.terminal_at_ms(), Some(110));
    assert_eq!(task.updated_at_ms(), 120);
}

#[test]
fn pause_records_target_and_resume_requires_exact_target() {
    let mut task = worktree_ready_task();
    task.pause(104).expect("WorktreeReady can pause");
    assert_eq!(task.state(), TaskState::Paused);
    assert_eq!(task.resume_target_state(), Some(TaskState::WorktreeReady));
    assert!(task.state().requires_active_lease());
    assert!(!TaskState::WorktreeReady.can_transition_to(TaskState::Paused));
    assert!(TaskState::WorktreeReady.can_contextually_transition_to(TaskState::Paused));

    let before = task.clone();
    assert_eq!(
        task.resume_from_pause(
            TaskState::Testing,
            ResumeValidation::from_completed_checks(),
            105,
        ),
        Err(DomainError::InvalidStateTransition)
    );
    assert_eq!(task, before);

    task.resume_from_pause(
        TaskState::WorktreeReady,
        ResumeValidation::from_completed_checks(),
        105,
    )
    .expect("verified exact target resumes");
    assert_eq!(task.state(), TaskState::WorktreeReady);
    assert_eq!(task.resume_target_state(), None);
    assert_eq!(task.version(), 5);
}

#[test]
fn static_pause_exit_to_recovery_clears_resume_target() {
    let mut task = worktree_ready_task();
    task.pause(104).expect("pause");
    task.transition_to(TaskState::RecoveryRequired, 105)
        .expect("Paused -> RecoveryRequired");
    assert_eq!(task.state(), TaskState::RecoveryRequired);
    assert_eq!(task.resume_target_state(), None);
}

#[test]
fn terminal_states_cannot_pause() {
    let mut task = new_task(100);
    task.transition_to(TaskState::Failed, 101)
        .expect("Created -> Failed");
    let before = task.clone();
    assert_eq!(task.pause(102), Err(DomainError::InvalidStateTransition));
    assert_eq!(task, before);
}

#[test]
fn recovery_target_requires_validation_and_rejects_special_or_terminal_states() {
    let mut task = recovery_required_task();
    assert_eq!(task.resume_target_state(), None);
    assert!(task.state().requires_active_lease());
    assert!(!TaskState::RecoveryRequired.can_transition_to(TaskState::Paused));

    for invalid in [
        TaskState::Paused,
        TaskState::RecoveryRequired,
        TaskState::UnknownExternalEffect,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::CleanupPending,
        TaskState::Archived,
    ] {
        assert_eq!(
            task.set_recovery_target(invalid, RecoveryValidation::from_completed_checks()),
            Err(DomainError::InvalidStateTransition)
        );
        assert_eq!(task.resume_target_state(), None);
    }

    // There is no target-setting API that omits the opaque validation token.
    task.set_recovery_target(
        TaskState::Testing,
        RecoveryValidation::from_completed_checks(),
    )
    .expect("verified normal workflow target");
    assert_eq!(task.resume_target_state(), Some(TaskState::Testing));
}

#[test]
fn recovery_can_pause_only_with_verified_existing_target_and_keeps_it() {
    let mut without_target = recovery_required_task();
    let before = without_target.clone();
    assert_eq!(
        without_target.pause_from_recovery(RecoveryValidation::from_completed_checks(), 104,),
        Err(DomainError::InvalidStateTransition)
    );
    assert_eq!(without_target, before);

    let mut task = recovery_required_task();
    task.set_recovery_target(
        TaskState::Testing,
        RecoveryValidation::from_completed_checks(),
    )
    .expect("set target");
    task.pause_from_recovery(RecoveryValidation::from_completed_checks(), 104)
        .expect("verified recovery target permits pause");
    assert_eq!(task.state(), TaskState::Paused);
    assert_eq!(task.resume_target_state(), Some(TaskState::Testing));
    assert_eq!(task.version(), 4);
}

#[test]
fn recovery_resume_clears_target_and_rejects_mismatch_atomically() {
    let mut task = recovery_required_task();
    task.set_recovery_target(
        TaskState::Testing,
        RecoveryValidation::from_completed_checks(),
    )
    .expect("set target");

    let before = task.clone();
    assert_eq!(
        task.resume_from_recovery(
            TaskState::ImplementingWithCodex,
            RecoveryValidation::from_completed_checks(),
            104,
        ),
        Err(DomainError::InvalidStateTransition)
    );
    assert_eq!(task, before);

    task.resume_from_recovery(
        TaskState::Testing,
        RecoveryValidation::from_completed_checks(),
        104,
    )
    .expect("matching recovery target resumes");
    assert_eq!(task.state(), TaskState::Testing);
    assert_eq!(task.resume_target_state(), None);
    assert_eq!(task.version(), 4);
}

#[test]
fn unknown_external_effect_has_no_target_and_only_uses_recovery_path() {
    let base = new_task(100);
    let mut snapshot = base.snapshot();
    snapshot.state = TaskState::UnknownExternalEffect;
    snapshot.version = 1;
    let mut task = Task::restore(snapshot).expect("valid unknown-effect snapshot");

    assert_eq!(task.resume_target_state(), None);
    assert!(!TaskState::UnknownExternalEffect.can_transition_to(TaskState::Paused));
    assert!(!TaskState::UnknownExternalEffect.can_contextually_transition_to(TaskState::Paused));
    assert!(TaskState::UnknownExternalEffect.can_transition_to(TaskState::RecoveryRequired));
    assert_eq!(task.pause(101), Err(DomainError::InvalidStateTransition));

    task.transition_to(TaskState::RecoveryRequired, 101)
        .expect("unknown effect must first enter recovery");
    task.set_recovery_target(
        TaskState::Testing,
        RecoveryValidation::from_completed_checks(),
    )
    .expect("recovery analysis sets a target");
    task.resume_from_recovery(
        TaskState::Testing,
        RecoveryValidation::from_completed_checks(),
        102,
    )
    .expect("verified recovery resumes work");
    assert_eq!(task.state(), TaskState::Testing);
}

#[test]
fn restore_rejects_invalid_state_target_timestamp_version_and_identity_combinations() {
    let task = new_task(100);

    let mut paused_without_target = task.snapshot();
    paused_without_target.state = TaskState::Paused;
    paused_without_target.version = 1;
    assert_eq!(
        Task::restore(paused_without_target),
        Err(DomainError::InvariantViolation)
    );

    let mut ordinary_with_target = task.snapshot();
    ordinary_with_target.resume_target_state = Some(TaskState::Testing);
    assert_eq!(
        Task::restore(ordinary_with_target),
        Err(DomainError::InvariantViolation)
    );

    let mut terminal_with_target = task.snapshot();
    terminal_with_target.state = TaskState::Failed;
    terminal_with_target.version = 1;
    terminal_with_target.resume_target_state = Some(TaskState::Testing);
    terminal_with_target.terminal_at_ms = Some(100);
    assert_eq!(
        Task::restore(terminal_with_target),
        Err(DomainError::InvariantViolation)
    );

    let mut unknown_with_target = task.snapshot();
    unknown_with_target.state = TaskState::UnknownExternalEffect;
    unknown_with_target.version = 1;
    unknown_with_target.resume_target_state = Some(TaskState::Testing);
    assert_eq!(
        Task::restore(unknown_with_target),
        Err(DomainError::InvariantViolation)
    );

    let mut reversed_time = task.snapshot();
    reversed_time.updated_at_ms = 99;
    assert_eq!(
        Task::restore(reversed_time),
        Err(DomainError::InvalidTimestamp)
    );

    let mut invalid_version = task.snapshot();
    invalid_version.state = TaskState::Testing;
    assert_eq!(
        Task::restore(invalid_version),
        Err(DomainError::InvalidVersion)
    );

    let mut wrong_branch = task.snapshot();
    wrong_branch.task_branch_identity = TaskBranchIdentity::for_task(TaskId::new());
    assert_eq!(
        Task::restore(wrong_branch),
        Err(DomainError::InvariantViolation)
    );
}

fn transition_snapshot(
    sequence: u64,
    from_state: Option<TaskState>,
    to_state: TaskState,
    task_version: u64,
) -> TaskStateTransitionSnapshot {
    TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id: TaskId::new(),
        sequence,
        from_state,
        to_state,
        task_version,
        actor_kind: ActorKind::from_str("application.user").expect("valid actor"),
        reason_code: ReasonCode::from_str("task.created").expect("valid reason"),
        occurred_at_ms: 100,
    }
}

#[test]
fn transition_sequence_and_initial_state_rules_are_enforced() {
    let transition_id = TaskStateTransitionId::new();
    let task_id = TaskId::new();
    let actor_kind = ActorKind::from_str("application").expect("valid actor");
    let reason_code = ReasonCode::from_str("task.created").expect("valid reason");
    let initial = TaskStateTransition::initial(
        transition_id,
        task_id,
        actor_kind.clone(),
        reason_code.clone(),
        100,
    );
    assert_eq!(initial.id(), transition_id);
    assert_eq!(initial.task_id(), task_id);
    assert_eq!(initial.sequence(), 1);
    assert_eq!(initial.from_state(), None);
    assert_eq!(initial.to_state(), TaskState::Created);
    assert_eq!(initial.task_version(), 0);
    assert_eq!(initial.actor_kind(), &actor_kind);
    assert_eq!(initial.reason_code(), &reason_code);
    assert_eq!(initial.occurred_at_ms(), 100);

    assert!(
        TaskStateTransition::new(transition_snapshot(
            2,
            Some(TaskState::Created),
            TaskState::ProjectValidated,
            1,
        ))
        .is_ok()
    );
    assert_eq!(
        TaskStateTransition::new(transition_snapshot(
            0,
            Some(TaskState::Created),
            TaskState::ProjectValidated,
            1,
        )),
        Err(DomainError::InvalidVersion)
    );
    assert_eq!(
        TaskStateTransition::new(transition_snapshot(2, None, TaskState::ProjectValidated, 1,)),
        Err(DomainError::InvariantViolation)
    );
    assert_eq!(TaskStateTransition::checked_next_sequence(1), Ok(2));
    assert_eq!(
        TaskStateTransition::checked_next_sequence(u64::MAX),
        Err(DomainError::InvalidVersion)
    );
    // Checking previous + 1 against repository history is intentionally a repository concern.
}

#[test]
fn actor_and_reason_are_bounded_safe_persistence_codes() {
    assert!(ActorKind::from_str("application.user-1").is_ok());
    assert!(ReasonCode::from_str("task.pause:approved").is_ok());

    for invalid in ["", "has space", "한글", "slash/value"] {
        assert_eq!(
            ActorKind::from_str(invalid),
            Err(DomainError::InvalidActorKind)
        );
        assert_eq!(
            ReasonCode::from_str(invalid),
            Err(DomainError::InvalidReasonCode)
        );
    }
    assert_eq!(
        ActorKind::from_str(&"a".repeat(ACTOR_KIND_MAX_LENGTH + 1)),
        Err(DomainError::InvalidActorKind)
    );
    assert_eq!(
        ReasonCode::from_str(&"r".repeat(REASON_CODE_MAX_LENGTH + 1)),
        Err(DomainError::InvalidReasonCode)
    );
}
