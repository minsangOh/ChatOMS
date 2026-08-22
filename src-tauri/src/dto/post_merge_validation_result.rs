use chatoms_application::tasks::PostMergeValidationResultView;
use chatoms_ports::repository::PostMergeValidationResultOutcome;
use serde::Serialize;

use super::ValidationCommandKindDto;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PostMergeValidationOutcomeDto {
    Success,
    ExitFailure,
    TimedOut,
    StdoutBoundExceeded,
    BindingRejected,
    Cancelled,
    Uncertain,
}

impl From<PostMergeValidationResultOutcome> for PostMergeValidationOutcomeDto {
    fn from(value: PostMergeValidationResultOutcome) -> Self {
        match value {
            PostMergeValidationResultOutcome::Success => Self::Success,
            PostMergeValidationResultOutcome::ExitFailure => Self::ExitFailure,
            PostMergeValidationResultOutcome::TimedOut => Self::TimedOut,
            PostMergeValidationResultOutcome::StdoutBoundExceeded => Self::StdoutBoundExceeded,
            PostMergeValidationResultOutcome::BindingRejected => Self::BindingRejected,
            PostMergeValidationResultOutcome::Cancelled => Self::Cancelled,
            PostMergeValidationResultOutcome::Uncertain => Self::Uncertain,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMergeValidationResultDto {
    pub command_kind: ValidationCommandKindDto,
    pub attempt_sequence: u32,
    pub outcome: PostMergeValidationOutcomeDto,
    pub exit_code: Option<i32>,
    pub safe_summary: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}

impl From<PostMergeValidationResultView> for PostMergeValidationResultDto {
    fn from(value: PostMergeValidationResultView) -> Self {
        Self {
            command_kind: value.kind.into(),
            attempt_sequence: value.attempt_sequence,
            outcome: value.outcome.into(),
            exit_code: value.exit_code,
            safe_summary: value.safe_summary,
            started_at_ms: value.started_at_ms,
            completed_at_ms: value.completed_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use chatoms_domain::ValidationCommandKind;
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn result_dto_serializes_only_content_free_safe_fields() {
        let dto = PostMergeValidationResultDto::from(PostMergeValidationResultView {
            kind: ValidationCommandKind::Test,
            attempt_sequence: 1,
            outcome: PostMergeValidationResultOutcome::Success,
            exit_code: Some(0),
            safe_summary: "post-merge validation completed successfully".to_owned(),
            started_at_ms: 10,
            completed_at_ms: 20,
        });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize result DTO") else {
            panic!("expected JSON response");
        };
        assert_eq!(
            json,
            "{\"commandKind\":\"test\",\"attemptSequence\":1,\"outcome\":\"success\",\"exitCode\":0,\"safeSummary\":\"post-merge validation completed successfully\",\"startedAtMs\":10,\"completedAtMs\":20}"
        );
        assert!(!json.contains("stdout") && !json.contains("path"));
    }
}
