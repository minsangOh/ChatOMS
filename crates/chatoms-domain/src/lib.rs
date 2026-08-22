#![doc = "Domain boundary for ChatOMS business rules and invariant-bearing types."]
#![forbid(unsafe_code)]

mod context_data_scope;
mod error;
mod high_risk_category;
mod id;
mod operation_risk;
mod task;
mod task_brief;
mod task_state;
mod transition;
mod validation_command;
mod work_kind;

pub use context_data_scope::ContextDataScope;
pub use error::DomainError;
pub use high_risk_category::HighRiskCategory;
pub use id::{
    AppProfileId, GitOperationId, ProjectId, ProviderBindingId, TaskId, TaskStateTransitionId,
};
pub use operation_risk::{OperationRiskKind, TargetIdentityDigest};
pub use task::{RecoveryValidation, ResumeValidation, Task, TaskBranchIdentity, TaskSnapshot};
pub use task_brief::TaskBrief;
pub use task_state::TaskState;
pub use transition::{
    ACTOR_KIND_MAX_LENGTH, ActorKind, REASON_CODE_MAX_LENGTH, ReasonCode, TaskStateTransition,
    TaskStateTransitionSnapshot,
};
pub use validation_command::{ValidationCommandKind, ValidationExecutionScope};
pub use work_kind::WorkKind;
