//! Strict provenance resolver for the approval version a task's merge
//! lifecycle is bound to: the `task_version` the task had when it first
//! entered `AwaitingUserDiffApproval`, which is the immutable
//! `approved_task_version` its diff approval and its `ProjectRoot`
//! validation approvals were recorded under.
//!
//! Both entry points accept exactly one chain shape, anchored at the
//! transition that *entered* `AwaitingUserDiffApproval` and ending at the
//! transition that produced the task's current state and version:
//!
//! ```text
//! * -> AwaitingUserDiffApproval
//! AwaitingUserDiffApproval -> Merging
//! ( Merging -> MergeConflict , MergeConflict -> Merging )*
//! Merging -> <target>
//! ```
//!
//! `<target>` is `MergeConflict` for
//! [`resolve_merge_conflict_approval_version`] and `PostMergeTesting` for
//! [`resolve_post_merge_approval_version`]. The `( ... )*` group is what
//! makes repeated conflict rounds resolvable: a merge that conflicts, is
//! manually resolved, is continued, and conflicts again stays inside the
//! same merge lifecycle and still resolves to the same first
//! `AwaitingUserDiffApproval` version.
//!
//! Every link is verified in both directions: consecutive `sequence`,
//! consecutive `task_version`, matching `task_id`, and the predecessor's
//! `to_state` equal to the successor's `from_state`. Anything else — an
//! unrelated history, a version gap, a reordered or partial chain, a
//! different task's transitions, a chain that does not end at the task's
//! current version — resolves to `None`, which every caller treats as
//! fail-closed. There is no latest-approval fallback and no
//! `current_version - N` arithmetic.

use chatoms_domain::{Task, TaskState, TaskStateTransition};

/// Resolves the approval version for a task currently in `MergeConflict`.
#[must_use]
pub fn resolve_merge_conflict_approval_version(
    transitions: &[TaskStateTransition],
    task: &Task,
) -> Option<u64> {
    resolve_merge_chain(transitions, task, TaskState::MergeConflict)
}

/// Resolves the approval version for a task currently in
/// `PostMergeTesting`.
#[must_use]
pub fn resolve_post_merge_approval_version(
    transitions: &[TaskStateTransition],
    task: &Task,
) -> Option<u64> {
    resolve_merge_chain(transitions, task, TaskState::PostMergeTesting)
}

/// Walks the chain backwards from the transition that produced the task's
/// current state and version. Walking backwards (rather than matching a
/// fixed-width window forwards) is what lets one implementation accept any
/// number of conflict rounds without ever relaxing a single link check.
fn resolve_merge_chain(
    transitions: &[TaskStateTransition],
    task: &Task,
    target_state: TaskState,
) -> Option<u64> {
    // `task_version` is the version *after* a transition and increases by
    // exactly one per transition, so at most one transition can carry the
    // task's current version.
    let end = transitions.iter().position(|transition| {
        transition.task_id() == task.id() && transition.task_version() == task.version()
    })?;
    if transitions[end].to_state() != target_state
        || transitions[end].from_state() != Some(TaskState::Merging)
    {
        return None;
    }

    // Loop invariant: `transitions[index].from_state() == Some(Merging)`,
    // so its predecessor must be the transition that entered `Merging`.
    // Each iteration moves `index` back by at least two, so this
    // terminates.
    let mut index = end;
    loop {
        let entered_merging_at = index.checked_sub(1)?;
        let entered_merging = linked_predecessor(transitions, index, task)?;
        match entered_merging.from_state() {
            // `AwaitingUserDiffApproval -> Merging`: the first merge
            // attempt of this lifecycle. Its own predecessor is the anchor.
            Some(TaskState::AwaitingUserDiffApproval) => {
                let anchor = linked_predecessor(transitions, entered_merging_at, task)?;
                // Redundant with `linked_predecessor`'s state link, kept
                // explicit because this value is the returned approval
                // version.
                if anchor.to_state() != TaskState::AwaitingUserDiffApproval {
                    return None;
                }
                return Some(anchor.task_version());
            }
            // `MergeConflict -> Merging`: a resumed merge. The transition
            // before it must be the `Merging -> MergeConflict` that opened
            // that conflict round, restoring the loop invariant.
            Some(TaskState::MergeConflict) => {
                let opened_conflict_at = entered_merging_at.checked_sub(1)?;
                let opened_conflict = linked_predecessor(transitions, entered_merging_at, task)?;
                if opened_conflict.to_state() != TaskState::MergeConflict
                    || opened_conflict.from_state() != Some(TaskState::Merging)
                {
                    return None;
                }
                index = opened_conflict_at;
            }
            _ => return None,
        }
    }
}

/// Returns `transitions[index - 1]` only when it is this task's immediately
/// preceding transition in every respect: adjacent in the slice,
/// consecutive in `sequence` and `task_version`, same `task_id`, and its
/// `to_state` equal to `transitions[index]`'s `from_state`.
fn linked_predecessor<'a>(
    transitions: &'a [TaskStateTransition],
    index: usize,
    task: &Task,
) -> Option<&'a TaskStateTransition> {
    let current = &transitions[index];
    let previous = transitions.get(index.checked_sub(1)?)?;
    (previous.task_id() == task.id()
        && current.task_id() == task.id()
        && previous.sequence().checked_add(1) == Some(current.sequence())
        && previous.task_version().checked_add(1) == Some(current.task_version())
        && current.from_state() == Some(previous.to_state()))
    .then_some(previous)
}
