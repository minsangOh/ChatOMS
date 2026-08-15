use chatoms_application::tasks::ContextPackageImplementationReadiness;
use serde::Serialize;

/// Content-free read-only readiness signal: whether an exact `(task_id,
/// Claude, Implementation, expected_version, ContextPackageV1)` consent and
/// its FK-bound manifest already exist. Carries no consent/manifest value,
/// no timestamp, and no task identity beyond what the caller already
/// supplied as the request — this command never creates, reuses, or
/// mutates anything. Deliberately says nothing about whether a completed
/// stored Claude Planning result exists — that is a separate structural
/// precondition checked only when actually starting Implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPackageImplementationReadinessDto {
    pub ready: bool,
}

impl From<ContextPackageImplementationReadiness> for ContextPackageImplementationReadinessDto {
    fn from(value: ContextPackageImplementationReadiness) -> Self {
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
            ContextPackageImplementationReadinessDto::from(ContextPackageImplementationReadiness {
                ready: true,
            });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize readiness DTO") else {
            panic!("expected JSON response");
        };
        assert_eq!(json, "{\"ready\":true}");
    }
}
