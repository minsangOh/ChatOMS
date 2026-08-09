#![doc = "Domain boundary for ChatOMS business rules and invariant-bearing types."]
#![forbid(unsafe_code)]

mod error;
mod id;
mod task;
mod task_state;
mod transition;

pub use error::DomainError;
pub use id::{
    AppProfileId, GitOperationId, ProjectId, ProviderBindingId, TaskId, TaskStateTransitionId,
};
pub use task::{RecoveryValidation, ResumeValidation, Task, TaskBranchIdentity, TaskSnapshot};
pub use task_state::TaskState;
pub use transition::{
    ACTOR_KIND_MAX_LENGTH, ActorKind, REASON_CODE_MAX_LENGTH, ReasonCode, TaskStateTransition,
    TaskStateTransitionSnapshot,
};
