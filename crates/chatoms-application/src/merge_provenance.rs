//! Strict provenance resolver for the `ProjectRoot` approval version a
//! `PostMergeTesting` task's approved validation commands and diff approval
//! must be looked up under. Accepts exactly two immutable transition
//! chains, both anchored at the task's first `AwaitingUserDiffApproval`
//! transition:
//!
//! - direct: `AwaitingUserDiffApproval -> Merging -> PostMergeTesting`
//! - conflict-resolved: `AwaitingUserDiffApproval -> Merging ->
//!   MergeConflict -> Merging -> PostMergeTesting`
//!
//! Every transition in the matched chain must be consecutive in both
//! `sequence` and `task_version`, and the chain's last transition's
//! `task_version` must equal the task's current version. No other chain
//! shape is accepted — no latest-approval fallback, no `current_version -
//! N` arithmetic, no partial or reordered chain.

use chatoms_domain::{Task, TaskState, TaskStateTransition};

#[must_use]
pub fn resolve_post_merge_approval_version(
    transitions: &[TaskStateTransition],
    task: &Task,
) -> Option<u64> {
    resolve_direct_chain(transitions, task)
        .or_else(|| resolve_conflict_resolved_chain(transitions, task))
}

fn consecutive(chain: &[&TaskStateTransition], task: &Task) -> bool {
    chain
        .iter()
        .all(|transition| transition.task_id() == task.id())
        && chain.windows(2).all(|pair| {
            pair[0].sequence().checked_add(1) == Some(pair[1].sequence())
                && pair[0].task_version().checked_add(1) == Some(pair[1].task_version())
        })
}

fn resolve_direct_chain(transitions: &[TaskStateTransition], task: &Task) -> Option<u64> {
    transitions.windows(3).find_map(|chain| {
        let refs: Vec<&TaskStateTransition> = chain.iter().collect();
        (consecutive(&refs, task)
            && chain[0].to_state() == TaskState::AwaitingUserDiffApproval
            && chain[1].from_state() == Some(TaskState::AwaitingUserDiffApproval)
            && chain[1].to_state() == TaskState::Merging
            && chain[2].from_state() == Some(TaskState::Merging)
            && chain[2].to_state() == TaskState::PostMergeTesting
            && chain[2].task_version() == task.version())
        .then_some(chain[0].task_version())
    })
}

fn resolve_conflict_resolved_chain(
    transitions: &[TaskStateTransition],
    task: &Task,
) -> Option<u64> {
    transitions.windows(5).find_map(|chain| {
        let refs: Vec<&TaskStateTransition> = chain.iter().collect();
        (consecutive(&refs, task)
            && chain[0].to_state() == TaskState::AwaitingUserDiffApproval
            && chain[1].from_state() == Some(TaskState::AwaitingUserDiffApproval)
            && chain[1].to_state() == TaskState::Merging
            && chain[2].from_state() == Some(TaskState::Merging)
            && chain[2].to_state() == TaskState::MergeConflict
            && chain[3].from_state() == Some(TaskState::MergeConflict)
            && chain[3].to_state() == TaskState::Merging
            && chain[4].from_state() == Some(TaskState::Merging)
            && chain[4].to_state() == TaskState::PostMergeTesting
            && chain[4].task_version() == task.version())
        .then_some(chain[0].task_version())
    })
}
