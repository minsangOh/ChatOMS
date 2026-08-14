use chatoms_application::tasks::ReviewResultView;
use chatoms_ports::repository::ReviewResultOutcome;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewOutcomeDto {
    Completed,
    Failed,
    Cancelled,
    RecoveryRequired,
}

impl From<ReviewResultOutcome> for ReviewOutcomeDto {
    fn from(value: ReviewResultOutcome) -> Self {
        match value {
            ReviewResultOutcome::Completed => Self::Completed,
            ReviewResultOutcome::Failed => Self::Failed,
            ReviewResultOutcome::Cancelled => Self::Cancelled,
            ReviewResultOutcome::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}

/// Read-only, already-safe Claude Review result. `review_text` is exactly
/// the masked, size-bounded text persisted by `record_review_result` — this
/// DTO never re-masks, re-parses, or re-runs anything. Never carries the raw
/// Git diff, raw provider transcript, stdout/stderr, tool I/O, session/login
/// information, or an executable path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResultDto {
    pub outcome: ReviewOutcomeDto,
    pub exit_code: Option<i32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub review_text: Option<String>,
}

impl From<ReviewResultView> for ReviewResultDto {
    fn from(value: ReviewResultView) -> Self {
        Self {
            outcome: value.outcome.into(),
            exit_code: value.exit_code,
            turn_count: value.turn_count,
            started_at_ms: value.started_at_ms,
            completed_at_ms: value.completed_at_ms,
            review_text: value.review_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn review_result_dto_serializes_only_the_safe_stored_fields() {
        let dto = ReviewResultDto::from(ReviewResultView {
            outcome: ReviewResultOutcome::Completed,
            exit_code: Some(0),
            turn_count: Some(3),
            started_at_ms: 10,
            completed_at_ms: 20,
            review_text: Some("The change matches the requirements.".to_owned()),
        });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize review result DTO")
        else {
            panic!("expected JSON response");
        };
        assert_eq!(
            json,
            "{\"outcome\":\"completed\",\"exitCode\":0,\"turnCount\":3,\"startedAtMs\":10,\"completedAtMs\":20,\"reviewText\":\"The change matches the requirements.\"}"
        );
    }
}
