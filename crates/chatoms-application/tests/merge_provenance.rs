use chatoms_application::merge_provenance::{
    resolve_merge_conflict_approval_version, resolve_post_merge_approval_version,
};
use chatoms_domain::{
    ActorKind, ReasonCode, TaskId, TaskState, TaskStateTransition, TaskStateTransitionId,
    TaskStateTransitionSnapshot,
};

fn transition(
    task_id: TaskId,
    sequence: u64,
    from_state: Option<TaskState>,
    to_state: TaskState,
    task_version: u64,
) -> TaskStateTransition {
    TaskStateTransition::new(TaskStateTransitionSnapshot {
        id: TaskStateTransitionId::new(),
        task_id,
        sequence,
        from_state,
        to_state,
        task_version,
        actor_kind: "test.actor".parse::<ActorKind>().expect("actor"),
        reason_code: "test.reason".parse::<ReasonCode>().expect("reason"),
        occurred_at_ms: 10 + sequence as i64,
    })
    .expect("transition snapshot")
}

fn task_at(state: TaskState, version: u64) -> chatoms_domain::Task {
    let id = TaskId::new();
    chatoms_domain::Task::restore(chatoms_domain::TaskSnapshot {
        id,
        project_id: chatoms_domain::ProjectId::new(),
        state,
        version,
        task_branch_identity: chatoms_domain::TaskBranchIdentity::for_task(id),
        resume_target_state: None,
        created_at_ms: 10,
        updated_at_ms: 20,
        terminal_at_ms: None,
    })
    .expect("restored task")
}

#[test]
fn direct_chain_resolves_the_first_awaiting_diff_approval_version() {
    let task = task_at(TaskState::PostMergeTesting, 3);
    let task_id = task.id();
    let transitions = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            2,
        ),
        transition(
            task_id,
            4,
            Some(TaskState::Merging),
            TaskState::PostMergeTesting,
            3,
        ),
    ];
    assert_eq!(
        resolve_post_merge_approval_version(&transitions, &task),
        Some(1)
    );
}

#[test]
fn conflict_resolved_chain_resolves_the_first_awaiting_diff_approval_version() {
    let task = task_at(TaskState::PostMergeTesting, 5);
    let task_id = task.id();
    let transitions = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            2,
        ),
        transition(
            task_id,
            4,
            Some(TaskState::Merging),
            TaskState::MergeConflict,
            3,
        ),
        transition(
            task_id,
            5,
            Some(TaskState::MergeConflict),
            TaskState::Merging,
            4,
        ),
        transition(
            task_id,
            6,
            Some(TaskState::Merging),
            TaskState::PostMergeTesting,
            5,
        ),
    ];
    assert_eq!(
        resolve_post_merge_approval_version(&transitions, &task),
        Some(1)
    );
}

#[test]
fn malformed_nonconsecutive_or_partial_chains_are_rejected() {
    // Missing the middle Merging hop entirely (partial chain).
    let task = task_at(TaskState::PostMergeTesting, 2);
    let task_id = task.id();
    let partial = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::PostMergeTesting,
            2,
        ),
    ];
    assert_eq!(resolve_post_merge_approval_version(&partial, &task), None);

    // Non-consecutive sequence numbers within an otherwise correct-looking chain.
    let task = task_at(TaskState::PostMergeTesting, 3);
    let task_id = task.id();
    let nonconsecutive = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            4,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            2,
        ),
        transition(
            task_id,
            5,
            Some(TaskState::Merging),
            TaskState::PostMergeTesting,
            3,
        ),
    ];
    assert_eq!(
        resolve_post_merge_approval_version(&nonconsecutive, &task),
        None
    );

    // Non-consecutive task_version within the chain.
    let task = task_at(TaskState::PostMergeTesting, 6);
    let task_id = task.id();
    let version_gap = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            5,
        ),
        transition(
            task_id,
            4,
            Some(TaskState::Merging),
            TaskState::PostMergeTesting,
            6,
        ),
    ];
    assert_eq!(
        resolve_post_merge_approval_version(&version_gap, &task),
        None
    );

    // Chain is otherwise perfectly consecutive, but ends at a task_version
    // that does not match the task's *current* version.
    let task = task_at(TaskState::PostMergeTesting, 9);
    let task_id = task.id();
    let stale_chain = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            2,
        ),
        transition(
            task_id,
            4,
            Some(TaskState::Merging),
            TaskState::PostMergeTesting,
            3,
        ),
    ];
    assert_eq!(
        resolve_post_merge_approval_version(&stale_chain, &task),
        None
    );

    // A different task's transitions must never be treated as this task's provenance,
    // even when every field would otherwise match.
    let task = task_at(TaskState::PostMergeTesting, 3);
    let other_task = TaskId::new();
    let foreign = vec![
        transition(other_task, 1, None, TaskState::Created, 0),
        transition(
            other_task,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            other_task,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            2,
        ),
        transition(
            other_task,
            4,
            Some(TaskState::Merging),
            TaskState::PostMergeTesting,
            3,
        ),
    ];
    assert_eq!(resolve_post_merge_approval_version(&foreign, &task), None);
}

/// Builds the transition history of a task that entered
/// `AwaitingUserDiffApproval` at version 1, started merging, then went
/// through `rounds` conflict rounds, and finally reached `final_state`
/// (`MergeConflict` when the last round is still open, `PostMergeTesting`
/// when the last continue succeeded). Sequence and `task_version` are both
/// strictly consecutive, exactly as the repository records them.
fn merge_lifecycle_history(
    task_id: TaskId,
    rounds: usize,
    final_state: TaskState,
) -> Vec<TaskStateTransition> {
    let mut history = vec![
        transition(task_id, 1, None, TaskState::Created, 0),
        transition(
            task_id,
            2,
            Some(TaskState::Created),
            TaskState::AwaitingUserDiffApproval,
            1,
        ),
        transition(
            task_id,
            3,
            Some(TaskState::AwaitingUserDiffApproval),
            TaskState::Merging,
            2,
        ),
    ];
    let push = |from: TaskState, to: TaskState, history: &mut Vec<TaskStateTransition>| {
        let sequence = history.len() as u64 + 1;
        let version = history.len() as u64;
        history.push(transition(task_id, sequence, Some(from), to, version));
    };
    for round in 0..rounds {
        let last = round + 1 == rounds;
        if last && final_state == TaskState::PostMergeTesting {
            push(
                TaskState::Merging,
                TaskState::PostMergeTesting,
                &mut history,
            );
        } else {
            push(TaskState::Merging, TaskState::MergeConflict, &mut history);
            if !(last && final_state == TaskState::MergeConflict) {
                push(TaskState::MergeConflict, TaskState::Merging, &mut history);
            }
        }
    }
    history
}

#[test]
fn repeated_conflict_rounds_still_resolve_the_first_awaiting_diff_approval_version() {
    // A `MergeConflict` task after two and after three conflict rounds: the
    // second and third rounds are reached by a real
    // `MergeConflict -> Merging -> MergeConflict` sequence, which the
    // original fixed-width resolver could not match at all — leaving such a
    // task with no resolvable provenance and therefore no way out.
    for rounds in 1..=3 {
        let task_id = TaskId::new();
        let history = merge_lifecycle_history(task_id, rounds, TaskState::MergeConflict);
        let last_version = history.last().expect("history is not empty").task_version();
        let task = task_at_id(task_id, TaskState::MergeConflict, last_version);
        assert_eq!(
            resolve_merge_conflict_approval_version(&history, &task),
            Some(1),
            "round {rounds}"
        );
        assert_eq!(
            resolve_post_merge_approval_version(&history, &task),
            None,
            "round {rounds}: a MergeConflict task must not resolve as post-merge"
        );
    }
}

#[test]
fn a_successful_continue_after_repeated_conflict_rounds_resolves_post_merge_provenance() {
    for rounds in 1..=3 {
        let task_id = TaskId::new();
        let history = merge_lifecycle_history(task_id, rounds, TaskState::PostMergeTesting);
        let last_version = history.last().expect("history is not empty").task_version();
        let task = task_at_id(task_id, TaskState::PostMergeTesting, last_version);
        assert_eq!(
            resolve_post_merge_approval_version(&history, &task),
            Some(1),
            "round {rounds}"
        );
        assert_eq!(
            resolve_merge_conflict_approval_version(&history, &task),
            None,
            "round {rounds}: a PostMergeTesting task must not resolve as merge-conflict"
        );
    }
}

#[test]
fn a_broken_link_inside_a_repeated_conflict_chain_is_still_rejected() {
    let task_id = TaskId::new();
    let mut history = merge_lifecycle_history(task_id, 3, TaskState::MergeConflict);
    let last_version = history.last().expect("history is not empty").task_version();
    let task = task_at_id(task_id, TaskState::MergeConflict, last_version);
    assert_eq!(
        resolve_merge_conflict_approval_version(&history, &task),
        Some(1),
        "the intact chain resolves, so the mutation below is what the assertions test"
    );

    // Break the version continuity of the second round's resumed merge.
    let broken = &history[4];
    history[4] = transition(
        task_id,
        broken.sequence(),
        broken.from_state(),
        broken.to_state(),
        broken.task_version() + 7,
    );
    assert_eq!(
        resolve_merge_conflict_approval_version(&history, &task),
        None,
        "a version gap anywhere in the chain must fail closed, not fall back to a later anchor"
    );

    // Restore, then break the state link instead: `MergeConflict -> Merging`
    // replaced by a state pair that never occurs in a merge lifecycle.
    let mut history = merge_lifecycle_history(task_id, 3, TaskState::MergeConflict);
    let replaced = &history[4];
    history[4] = transition(
        task_id,
        replaced.sequence(),
        Some(TaskState::MergeConflict),
        TaskState::Cancelled,
        replaced.task_version(),
    );
    assert_eq!(
        resolve_merge_conflict_approval_version(&history, &task),
        None,
        "a state-order violation must fail closed"
    );
}

fn task_at_id(id: TaskId, state: TaskState, version: u64) -> chatoms_domain::Task {
    chatoms_domain::Task::restore(chatoms_domain::TaskSnapshot {
        id,
        project_id: chatoms_domain::ProjectId::new(),
        state,
        version,
        task_branch_identity: chatoms_domain::TaskBranchIdentity::for_task(id),
        resume_target_state: None,
        created_at_ms: 10,
        updated_at_ms: 20,
        terminal_at_ms: None,
    })
    .expect("restored task")
}
