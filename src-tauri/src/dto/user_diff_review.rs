use chatoms_application::{tasks::DiffApprovalView, user_diff_approval::UserDiffForReview};
use serde::Serialize;

/// The ONLY DTO in this codebase that carries raw repository diff content.
/// It exists solely for `get_user_diff_for_review` to hand the diff, once,
/// directly to the requesting local user's own review modal — never to a
/// provider, never persisted, never logged. `Debug` deliberately hides
/// `diff_text` (only its byte length is shown), mirroring
/// `chatoms_ports::diff::WorktreeDiff` and
/// `chatoms_application::user_diff_approval::UserDiffForReview`, so a stray
/// `{:?}` in a log statement cannot leak diff content.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawUserDiffForReviewDto {
    pub diff_text: String,
    pub diff_content_hash: String,
}

impl std::fmt::Debug for RawUserDiffForReviewDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawUserDiffForReviewDto")
            .field("diff_text_byte_len", &self.diff_text.len())
            .field("diff_content_hash", &self.diff_content_hash)
            .finish()
    }
}

impl From<UserDiffForReview> for RawUserDiffForReviewDto {
    fn from(value: UserDiffForReview) -> Self {
        Self {
            diff_content_hash: value.diff_content_hash.to_hex(),
            diff_text: value.diff_text().to_owned(),
        }
    }
}

/// Content-free approval result: only the timestamp the approval was
/// recorded at. Never echoes back the task id, the diff content hash, or
/// any diff text — the caller already knows which task/version/hash it
/// asked to approve.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDiffApprovalDto {
    pub approved_at_ms: i64,
}

impl From<DiffApprovalView> for UserDiffApprovalDto {
    fn from(value: DiffApprovalView) -> Self {
        Self {
            approved_at_ms: value.approved_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use chatoms_ports::diff::DiffContentHash;
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    fn json(response: impl IpcResponse) -> String {
        let InvokeResponseBody::Json(json) = response.body().expect("JSON serialization") else {
            panic!("expected JSON response");
        };
        json
    }

    #[test]
    fn raw_diff_dto_serializes_diff_text_and_hex_digest() {
        let hash = DiffContentHash::from_digest_bytes([7u8; 32]);
        let dto = RawUserDiffForReviewDto {
            diff_text: "diff --git a/x b/x\n+line\n".to_owned(),
            diff_content_hash: hash.to_hex(),
        };
        let serialized = json(dto);
        assert!(serialized.contains("\"diffText\":\"diff --git a/x b/x\\n+line\\n\""));
        assert!(serialized.contains(&format!("\"diffContentHash\":\"{}\"", hash.to_hex())));
    }

    #[test]
    fn raw_diff_dto_debug_output_hides_the_diff_text() {
        let hash = DiffContentHash::from_digest_bytes([9u8; 32]);
        let dto = RawUserDiffForReviewDto {
            diff_text: "SECRET_LEAK_MARKER_must_never_appear_in_debug_output".to_owned(),
            diff_content_hash: hash.to_hex(),
        };
        let debug = format!("{dto:?}");
        assert!(!debug.contains("SECRET_LEAK_MARKER_must_never_appear_in_debug_output"));
        assert!(debug.contains("diff_text_byte_len"));
        assert!(debug.contains(&hash.to_hex()));
    }

    #[test]
    fn approval_dto_serializes_only_approved_at_ms() {
        let dto = UserDiffApprovalDto {
            approved_at_ms: 12345,
        };
        assert_eq!(json(dto), "{\"approvedAtMs\":12345}");
    }
}
