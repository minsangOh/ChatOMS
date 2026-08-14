use serde::{Deserialize, Serialize};

use crate::DomainError;

/// Immutable per-task brief: the requirements, completion criteria, and
/// prohibited scope captured once at task creation. There is no update path;
/// a new value can only be constructed, never mutated in place.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskBrief {
    requirements: String,
    completion_criteria: String,
    prohibited_scope: String,
}

impl TaskBrief {
    pub fn new(
        requirements: String,
        completion_criteria: String,
        prohibited_scope: String,
    ) -> Result<Self, DomainError> {
        if requirements.trim().is_empty()
            || completion_criteria.trim().is_empty()
            || prohibited_scope.trim().is_empty()
        {
            return Err(DomainError::InvalidTaskBrief);
        }
        Ok(Self {
            requirements,
            completion_criteria,
            prohibited_scope,
        })
    }

    #[must_use]
    pub fn requirements(&self) -> &str {
        &self.requirements
    }

    #[must_use]
    pub fn completion_criteria(&self) -> &str {
        &self.completion_criteria
    }

    #[must_use]
    pub fn prohibited_scope(&self) -> &str {
        &self.prohibited_scope
    }
}
