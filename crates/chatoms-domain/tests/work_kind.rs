use chatoms_domain::{TaskState, WorkKind};

#[test]
fn each_work_kind_has_exactly_one_task_state_that_allows_entry() {
    for (work_kind, entry_state) in [
        (WorkKind::Planning, TaskState::WorktreeReady),
        (WorkKind::Implementation, TaskState::AwaitingDesignApproval),
        (WorkKind::Review, TaskState::Reviewing),
    ] {
        assert_eq!(work_kind.entry_state(), entry_state);
        for state in TaskState::ALL {
            assert_eq!(
                work_kind.can_start_from(state),
                state == entry_state,
                "unexpected entry rule for {work_kind:?} from {state:?}"
            );
        }
    }
}

#[test]
fn work_kind_list_contains_only_the_three_provider_neutral_kinds() {
    assert_eq!(
        WorkKind::ALL,
        [
            WorkKind::Planning,
            WorkKind::Implementation,
            WorkKind::Review,
        ]
    );
}
