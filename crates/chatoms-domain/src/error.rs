use thiserror::Error;

#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum DomainError {
    #[error("invalid UUID")]
    InvalidUuid,
    #[error("unsupported UUID version")]
    UnsupportedUuidVersion,
    #[error("invalid task branch identity")]
    InvalidTaskBranchIdentity,
    #[error("invalid task state")]
    InvalidTaskState,
    #[error("invalid state transition")]
    InvalidStateTransition,
    #[error("invalid timestamp")]
    InvalidTimestamp,
    #[error("invalid version")]
    InvalidVersion,
    #[error("invalid actor kind")]
    InvalidActorKind,
    #[error("invalid reason code")]
    InvalidReasonCode,
    #[error("domain invariant violation")]
    InvariantViolation,
}
