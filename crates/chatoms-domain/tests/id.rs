mod support;

use std::str::FromStr;

use chatoms_domain::{
    AppProfileId, DomainError, ProjectId, ProviderBindingId, TaskBranchIdentity, TaskId,
    TaskStateTransitionId,
};
use serde::{Deserialize, Serialize, de::value};

use support::StringSerializer;

const UUID_V4: &str = "550e8400-e29b-41d4-a716-446655440000";

#[test]
fn every_id_type_generates_uuid_v7() {
    for value in [
        ProjectId::new().to_string(),
        AppProfileId::new().to_string(),
        ProviderBindingId::new().to_string(),
        TaskId::new().to_string(),
        TaskStateTransitionId::new().to_string(),
    ] {
        let parsed = uuid::Uuid::parse_str(&value).expect("generated ID must be a UUID");
        assert_eq!(parsed.get_version_num(), 7);
        assert_eq!(value, value.to_ascii_lowercase());
    }
}

#[test]
fn id_parse_canonicalizes_uppercase_and_rejects_invalid_values() {
    let original = TaskId::new();
    let uppercase = original.to_string().to_ascii_uppercase();
    let parsed = TaskId::from_str(&uppercase).expect("uppercase UUID input is accepted");
    assert_eq!(parsed, original);
    assert_eq!(parsed.to_string(), original.to_string());

    assert_eq!(
        TaskId::from_str(UUID_V4),
        Err(DomainError::UnsupportedUuidVersion)
    );
    assert_eq!(
        TaskId::from_str("not-a-uuid"),
        Err(DomainError::InvalidUuid)
    );
}

#[test]
fn id_serde_round_trip_uses_canonical_string() {
    let id = TaskId::new();
    let serialized = id
        .serialize(StringSerializer)
        .expect("string serializer must accept an ID");
    let deserializer = value::StrDeserializer::<value::Error>::new(&serialized);
    let deserialized = TaskId::deserialize(deserializer).expect("serialized ID must deserialize");
    assert_eq!(deserialized, id);
}

#[test]
fn task_branch_identity_is_derived_from_task_id() {
    let task_id = TaskId::new();
    let identity = TaskBranchIdentity::for_task(task_id);
    let expected = format!("ai-task/{task_id}");
    assert_eq!(identity.as_str(), expected);
    assert_eq!(TaskBranchIdentity::from_str(&expected), Ok(identity));
}

#[test]
fn task_branch_identity_rejects_invalid_prefix_version_case_and_whitespace() {
    let task_id = TaskId::new();
    let canonical = task_id.to_string();
    let uppercase = format!("ai-task/{}", canonical.to_ascii_uppercase());

    for invalid in [
        format!("task/{canonical}"),
        format!("ai-task/{UUID_V4}"),
        uppercase,
        format!("ai-task/{canonical} "),
    ] {
        assert_eq!(
            TaskBranchIdentity::from_str(&invalid),
            Err(DomainError::InvalidTaskBranchIdentity)
        );
    }
}
