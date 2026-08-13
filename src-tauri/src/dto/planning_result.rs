use chatoms_application::tasks::PlanningResultView;
use chatoms_ports::repository::PlanningResultOutcome;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanningOutcomeDto {
    Completed,
    Failed,
    Cancelled,
    RecoveryRequired,
}

impl From<PlanningResultOutcome> for PlanningOutcomeDto {
    fn from(value: PlanningResultOutcome) -> Self {
        match value {
            PlanningResultOutcome::Completed => Self::Completed,
            PlanningResultOutcome::Failed => Self::Failed,
            PlanningResultOutcome::Cancelled => Self::Cancelled,
            PlanningResultOutcome::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}

/// Read-only, already-safe Claude Planning result. `plan_text` is exactly
/// the masked, size-bounded text persisted by `record_planning_result` —
/// this DTO never re-masks, re-parses, or re-runs anything. Never carries
/// raw provider transcript, stdout/stderr, tool I/O, session/login
/// information, or an executable path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanningResultDto {
    pub outcome: PlanningOutcomeDto,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub plan_text: Option<String>,
}

impl From<PlanningResultView> for PlanningResultDto {
    fn from(value: PlanningResultView) -> Self {
        Self {
            outcome: value.outcome.into(),
            exit_code: value.exit_code,
            turn_count: value.turn_count,
            started_at_ms: value.started_at_ms,
            completed_at_ms: value.completed_at_ms,
            plan_text: value.plan_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn planning_result_dto_serializes_only_the_safe_stored_fields() {
        let dto = PlanningResultDto::from(PlanningResultView {
            outcome: PlanningResultOutcome::Completed,
            exit_code: Some(0),
            turn_count: Some(3),
            started_at_ms: 10,
            completed_at_ms: 20,
            plan_text: Some("Add a CSV export button.".to_owned()),
        });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize planning result DTO")
        else {
            panic!("expected JSON response");
        };
        assert_eq!(
            json,
            "{\"outcome\":\"completed\",\"exitCode\":0,\"turnCount\":3,\"startedAtMs\":10,\"completedAtMs\":20,\"planText\":\"Add a CSV export button.\"}"
        );
    }
}
