use chatoms_ports::repository::ContextPackagePreparation;
use serde::Serialize;

use super::WorkKindDto;

/// Fixed, single-variant data-scope vocabulary for this DTO. Always
/// constructed as `ContextPackageV1` regardless of the underlying stored
/// value — this conversion never reads or branches on
/// `ProviderConsent::data_scope`/`ContextPackageManifestRecord::data_scope`,
/// so there is no code path here that could parse or forward an arbitrary
/// scope string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextPackageDataScopeDto {
    ContextPackageV1,
}

/// Content-free confirmation that a `ContextPackageV1` consent and its
/// FK-bound manifest now exist for a task (created or reused — the two are
/// indistinguishable by design, since [`TaskService::prepare_planning_context_package`]
/// and its Implementation/Review siblings treat them identically). Carries
/// no `task_id` (the caller already has it), no `ProviderConsent`/
/// `ContextPackageManifestRecord` value directly, and no raw TaskBrief,
/// plan text, diff, validation summary, assembled payload, executable/
/// environment path, or login/session/cost information — this preparation
/// step never reads any of those in the first place.
///
/// [`TaskService::prepare_planning_context_package`]: chatoms_application::tasks::TaskService::prepare_planning_context_package
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPackagePreparationDto {
    pub work_kind: WorkKindDto,
    pub data_scope: ContextPackageDataScopeDto,
    pub consented_at_ms: i64,
    pub manifest_created_at_ms: i64,
}

impl From<ContextPackagePreparation> for ContextPackagePreparationDto {
    fn from(value: ContextPackagePreparation) -> Self {
        Self {
            work_kind: value.consent.work_kind.into(),
            data_scope: ContextPackageDataScopeDto::ContextPackageV1,
            consented_at_ms: value.consent.consented_at_ms,
            manifest_created_at_ms: value.manifest.created_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use chatoms_domain::WorkKind;
    use chatoms_domain::{ContextDataScope, TaskId};
    use chatoms_ports::{
        provider::ProviderKind,
        repository::{ContextPackageManifestRecord, ProviderConsent},
    };
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    fn preparation() -> ContextPackagePreparation {
        let task_id = TaskId::new();
        ContextPackagePreparation {
            consent: ProviderConsent {
                task_id,
                provider: ProviderKind::Claude,
                work_kind: WorkKind::Review,
                approved_task_version: 6,
                data_scope: ContextDataScope::ContextPackageV1,
                consented_at_ms: 200,
            },
            manifest: ContextPackageManifestRecord {
                task_id,
                provider: ProviderKind::Claude,
                work_kind: WorkKind::Review,
                approved_task_version: 6,
                data_scope: ContextDataScope::ContextPackageV1,
                created_at_ms: 210,
            },
        }
    }

    #[test]
    fn context_package_preparation_dto_serializes_only_content_free_fields() {
        let dto = ContextPackagePreparationDto::from(preparation());
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize preparation DTO") else {
            panic!("expected JSON response");
        };
        assert_eq!(
            json,
            "{\"workKind\":\"review\",\"dataScope\":\"contextPackageV1\",\"consentedAtMs\":200,\"manifestCreatedAtMs\":210}"
        );
        for forbidden in [
            "taskId",
            "requirements",
            "completionCriteria",
            "prohibitedScope",
            "planText",
            "diff",
            "reviewText",
            "safeSummary",
            "path",
            "executable",
            "session",
            "login",
            "cost",
        ] {
            assert!(!json.contains(forbidden), "unexpected field: {forbidden}");
        }
    }
}
