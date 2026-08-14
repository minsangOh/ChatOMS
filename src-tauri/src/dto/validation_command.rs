use chatoms_domain::ValidationCommandKind;
use chatoms_ports::validation::ValidationCommandCandidate;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationCommandKindDto {
    Format,
    Lint,
    Typecheck,
    Test,
    Build,
}

impl From<ValidationCommandKind> for ValidationCommandKindDto {
    fn from(value: ValidationCommandKind) -> Self {
        match value {
            ValidationCommandKind::Format => Self::Format,
            ValidationCommandKind::Lint => Self::Lint,
            ValidationCommandKind::Typecheck => Self::Typecheck,
            ValidationCommandKind::Test => Self::Test,
            ValidationCommandKind::Build => Self::Build,
        }
    }
}

impl From<ValidationCommandKindDto> for ValidationCommandKind {
    fn from(value: ValidationCommandKindDto) -> Self {
        match value {
            ValidationCommandKindDto::Format => Self::Format,
            ValidationCommandKindDto::Lint => Self::Lint,
            ValidationCommandKindDto::Typecheck => Self::Typecheck,
            ValidationCommandKindDto::Test => Self::Test,
            ValidationCommandKindDto::Build => Self::Build,
        }
    }
}

/// One Cargo-only discovered candidate, reduced to a `kind` plus a fixed,
/// hardcoded display label. Never carries the candidate's own
/// `executable`/`arguments`, the worktree path used to discover it, or any
/// manifest content — a future execution Unit's argv vocabulary is a backend
/// implementation detail, not something this read-only surface echoes back.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationCommandCandidateDto {
    pub kind: ValidationCommandKindDto,
    pub label: String,
}

impl ValidationCommandCandidateDto {
    #[must_use]
    pub fn from_cargo_candidate(candidate: &ValidationCommandCandidate) -> Self {
        Self {
            kind: candidate.kind.into(),
            label: fixed_label(candidate.kind).to_owned(),
        }
    }
}

fn fixed_label(kind: ValidationCommandKind) -> &'static str {
    match kind {
        ValidationCommandKind::Format => "Format (cargo fmt --check)",
        ValidationCommandKind::Lint => "Lint (cargo clippy)",
        ValidationCommandKind::Typecheck => "Typecheck",
        ValidationCommandKind::Test => "Test (cargo test)",
        ValidationCommandKind::Build => "Build (cargo build)",
    }
}

/// Read-only: which `ValidationCommandKind`s already have an approved,
/// immutable binding for the task's current version. Never carries the
/// stored executable path, tool directory, or `CARGO_HOME`/`RUSTUP_HOME`
/// path/identity — a caller that needs to re-verify a binding uses
/// `ValidationCommandService::verify_binding` server-side, not this DTO.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationCommandApprovalStatusDto {
    pub approved_kinds: Vec<ValidationCommandKindDto>,
}

/// Frontend-supplied approval input. `kinds` carries only which categories
/// the user selected — never an executable name or argv; the backend
/// re-derives those from the current Cargo candidates discovery proposes
/// right now, so the frontend can never choose or influence what actually
/// gets spawned later.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApproveValidationCommandInputDto {
    pub kinds: Vec<ValidationCommandKindDto>,
    pub executable_path: String,
    pub cargo_home_path: Option<String>,
    pub rustup_home_path: Option<String>,
}

/// Approval success response: which kinds are now approved for the task's
/// current version. Never echoes back the input path, its identity, or any
/// other detail of the binding that was just captured.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveValidationCommandResultDto {
    pub approved_kinds: Vec<ValidationCommandKindDto>,
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn candidate_dto_serializes_only_kind_and_label() {
        let dto =
            ValidationCommandCandidateDto::from_cargo_candidate(&ValidationCommandCandidate {
                kind: ValidationCommandKind::Test,
                executable: "cargo".to_owned(),
                arguments: vec!["test".to_owned(), "--workspace".to_owned()],
            });
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize candidate DTO") else {
            panic!("expected JSON response");
        };
        assert_eq!(json, "{\"kind\":\"test\",\"label\":\"Test (cargo test)\"}");
        assert!(
            !json.contains("arguments")
                && !json.contains("executable")
                && !json.contains("workspace"),
            "the candidate's own executable/arguments/worktree must never be echoed back"
        );
    }

    #[test]
    fn approval_status_dto_serializes_only_kinds() {
        let dto = ValidationCommandApprovalStatusDto {
            approved_kinds: vec![
                ValidationCommandKindDto::Format,
                ValidationCommandKindDto::Test,
            ],
        };
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize approval status DTO")
        else {
            panic!("expected JSON response");
        };
        assert_eq!(json, "{\"approvedKinds\":[\"format\",\"test\"]}");
    }

    #[test]
    fn approve_result_dto_serializes_only_approved_kinds() {
        let dto = ApproveValidationCommandResultDto {
            approved_kinds: vec![ValidationCommandKindDto::Lint],
        };
        let InvokeResponseBody::Json(json) = dto.body().expect("serialize approve result DTO")
        else {
            panic!("expected JSON response");
        };
        assert_eq!(json, "{\"approvedKinds\":[\"lint\"]}");
    }
}
