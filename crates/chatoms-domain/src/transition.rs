use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{DomainError, TaskId, TaskState, TaskStateTransitionId};

pub const ACTOR_KIND_MAX_LENGTH: usize = 64;
pub const REASON_CODE_MAX_LENGTH: usize = 128;

fn is_safe_code(value: &str, max_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

macro_rules! validated_code {
    ($name:ident, $max:ident, $error:expr) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if is_safe_code(value, $max) {
                    Ok(Self(value.to_owned()))
                } else {
                    Err($error)
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(D::Error::custom)
            }
        }
    };
}

validated_code!(
    ActorKind,
    ACTOR_KIND_MAX_LENGTH,
    DomainError::InvalidActorKind
);
validated_code!(
    ReasonCode,
    REASON_CODE_MAX_LENGTH,
    DomainError::InvalidReasonCode
);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskStateTransition {
    id: TaskStateTransitionId,
    task_id: TaskId,
    sequence: u64,
    from_state: Option<TaskState>,
    to_state: TaskState,
    task_version: u64,
    actor_kind: ActorKind,
    reason_code: ReasonCode,
    occurred_at_ms: i64,
}

impl TaskStateTransition {
    #[must_use]
    pub fn initial(
        id: TaskStateTransitionId,
        task_id: TaskId,
        actor_kind: ActorKind,
        reason_code: ReasonCode,
        occurred_at_ms: i64,
    ) -> Self {
        Self {
            id,
            task_id,
            sequence: 1,
            from_state: None,
            to_state: TaskState::Created,
            task_version: 0,
            actor_kind,
            reason_code,
            occurred_at_ms,
        }
    }

    pub fn new(snapshot: TaskStateTransitionSnapshot) -> Result<Self, DomainError> {
        if snapshot.sequence == 0 {
            return Err(DomainError::InvalidVersion);
        }
        match snapshot.from_state {
            None => {
                if snapshot.sequence != 1
                    || !matches!(snapshot.to_state, TaskState::Created)
                    || snapshot.task_version != 0
                {
                    return Err(DomainError::InvariantViolation);
                }
            }
            Some(_) => {
                if snapshot.task_version == 0 {
                    return Err(DomainError::InvalidVersion);
                }
            }
        }

        Ok(Self {
            id: snapshot.id,
            task_id: snapshot.task_id,
            sequence: snapshot.sequence,
            from_state: snapshot.from_state,
            to_state: snapshot.to_state,
            task_version: snapshot.task_version,
            actor_kind: snapshot.actor_kind,
            reason_code: snapshot.reason_code,
            occurred_at_ms: snapshot.occurred_at_ms,
        })
    }

    pub fn checked_next_sequence(previous: u64) -> Result<u64, DomainError> {
        if previous == 0 {
            return Err(DomainError::InvalidVersion);
        }
        previous.checked_add(1).ok_or(DomainError::InvalidVersion)
    }

    #[must_use]
    pub const fn id(&self) -> TaskStateTransitionId {
        self.id
    }

    #[must_use]
    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn from_state(&self) -> Option<TaskState> {
        self.from_state
    }

    #[must_use]
    pub const fn to_state(&self) -> TaskState {
        self.to_state
    }

    #[must_use]
    pub const fn task_version(&self) -> u64 {
        self.task_version
    }

    #[must_use]
    pub const fn actor_kind(&self) -> &ActorKind {
        &self.actor_kind
    }

    #[must_use]
    pub const fn reason_code(&self) -> &ReasonCode {
        &self.reason_code
    }

    #[must_use]
    pub const fn occurred_at_ms(&self) -> i64 {
        self.occurred_at_ms
    }
}

#[derive(Deserialize)]
pub struct TaskStateTransitionSnapshot {
    pub id: TaskStateTransitionId,
    pub task_id: TaskId,
    pub sequence: u64,
    pub from_state: Option<TaskState>,
    pub to_state: TaskState,
    pub task_version: u64,
    pub actor_kind: ActorKind,
    pub reason_code: ReasonCode,
    pub occurred_at_ms: i64,
}

impl<'de> Deserialize<'de> for TaskStateTransition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = TaskStateTransitionSnapshot::deserialize(deserializer)?;
        Self::new(snapshot).map_err(D::Error::custom)
    }
}
