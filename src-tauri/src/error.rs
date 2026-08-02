use std::fmt;

use chatoms_application::error::ApplicationError;
use chatoms_ports::error::{FailureCategory, FailureSeverity, RetryDisposition};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IpcSeverityDto {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IpcRetryDto {
    Never,
    Immediate,
    AfterUserAction,
    AfterStateRefresh,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcErrorDto {
    pub code: &'static str,
    pub message: &'static str,
    pub severity: IpcSeverityDto,
    pub retry: IpcRetryDto,
}

impl IpcErrorDto {
    #[must_use]
    pub fn internal() -> Self {
        ApplicationError::from_failure(
            FailureCategory::Internal,
            FailureCategory::Internal.default_severity(),
            FailureCategory::Internal.default_retry(),
        )
        .into()
    }

    #[must_use]
    pub fn not_found() -> Self {
        ApplicationError::from_failure(
            FailureCategory::NotFound,
            FailureCategory::NotFound.default_severity(),
            FailureCategory::NotFound.default_retry(),
        )
        .into()
    }
}

impl From<ApplicationError> for IpcErrorDto {
    fn from(error: ApplicationError) -> Self {
        Self {
            code: error.code().as_str(),
            message: error.user_message(),
            severity: error.severity().into(),
            retry: error.retry().into(),
        }
    }
}

impl From<FailureSeverity> for IpcSeverityDto {
    fn from(value: FailureSeverity) -> Self {
        match value {
            FailureSeverity::Info => Self::Info,
            FailureSeverity::Warning => Self::Warning,
            FailureSeverity::Error => Self::Error,
            FailureSeverity::Critical => Self::Critical,
        }
    }
}

impl From<RetryDisposition> for IpcRetryDto {
    fn from(value: RetryDisposition) -> Self {
        match value {
            RetryDisposition::Never => Self::Never,
            RetryDisposition::Immediate => Self::Immediate,
            RetryDisposition::AfterUserAction => Self::AfterUserAction,
            RetryDisposition::AfterStateRefresh => Self::AfterStateRefresh,
        }
    }
}

impl fmt::Display for IpcErrorDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

#[cfg(test)]
mod tests {
    use chatoms_ports::error::FailureCategory;
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    use super::*;

    #[test]
    fn application_error_conversion_exposes_only_safe_fields() {
        let application_error = ApplicationError::from_failure(
            FailureCategory::StorageUnavailable,
            FailureCategory::StorageUnavailable.default_severity(),
            FailureCategory::StorageUnavailable.default_retry(),
        );
        let error = IpcErrorDto::from(application_error);
        assert_eq!(error.code, "APP_STORAGE_UNAVAILABLE");
        assert_eq!(error.to_string(), "Secure local storage is unavailable.");
        let InvokeResponseBody::Json(json) = error.body().expect("serialized error") else {
            panic!("expected JSON response");
        };
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"retry\":\"afterUserAction\""));
        for forbidden in ["source", "SELECT", "S-1-", "C:\\\\", "secret"] {
            assert!(!json.contains(forbidden));
        }
    }
}
