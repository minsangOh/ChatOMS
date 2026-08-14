use serde::{Deserialize, Serialize};

use crate::TaskState;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum WorkKind {
    Planning,
    Implementation,
    Review,
}

impl WorkKind {
    pub const ALL: [Self; 3] = [Self::Planning, Self::Implementation, Self::Review];

    #[must_use]
    pub const fn entry_state(self) -> TaskState {
        match self {
            Self::Planning => TaskState::WorktreeReady,
            Self::Implementation => TaskState::AwaitingDesignApproval,
            // `Testing -> Reviewing` is already an automatic transition
            // (`TaskService::finalize_validation_command_batch` on full
            // validation success), so by the time a Claude Review run can
            // actually be started the task is already `Reviewing`, not
            // `Testing`.
            Self::Review => TaskState::Reviewing,
        }
    }

    #[must_use]
    pub const fn can_start_from(self, state: TaskState) -> bool {
        matches!(
            (self, state),
            (Self::Planning, TaskState::WorktreeReady)
                | (Self::Implementation, TaskState::AwaitingDesignApproval)
                | (Self::Review, TaskState::Reviewing)
        )
    }
}
