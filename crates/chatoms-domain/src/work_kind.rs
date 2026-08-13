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
            Self::Review => TaskState::Testing,
        }
    }

    #[must_use]
    pub const fn can_start_from(self, state: TaskState) -> bool {
        matches!(
            (self, state),
            (Self::Planning, TaskState::WorktreeReady)
                | (Self::Implementation, TaskState::AwaitingDesignApproval)
                | (Self::Review, TaskState::Testing)
        )
    }
}
