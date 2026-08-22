use chatoms_application::merge_provenance::resolve_post_merge_approval_version;
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
