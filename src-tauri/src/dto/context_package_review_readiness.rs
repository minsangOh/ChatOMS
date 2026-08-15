use chatoms_application::tasks::ContextPackageReviewReadiness;
use serde::Serialize;

/// Content-free read-only readiness signal: whether an exact `(task_id,
/// Claude, Review, expected_version, ContextPackageV1)` consent and its
/// FK-bound manifest already exist. Carries no consent/manifest value, no
/// timestamp, and no task identity beyond what the caller already supplied
/// as the request — this command never creates, reuses, or mutates
/// anything. Mirrors `ContextPackageImplementationReadinessDto` exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPackageReviewReadinessDto {
    pub ready: bool,
}

impl From<ContextPackageReviewReadiness> for ContextPackageReviewReadinessDto {
    fn from(value: ContextPackageReviewReadiness) -> Self {
        Self { ready: value.ready }
    }
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn readiness_dto_serializes_only_the_ready_field() {
        let dto =
            ContextPackageReviewReadinessDto::from(ContextPackageReviewReadiness { ready: true });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize readiness DTO") else {
            panic!("expected JSON response");
        };
        assert_eq!(json, "{\"ready\":true}");
    }
}
