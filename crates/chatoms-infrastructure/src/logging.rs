use std::{fmt, path::PathBuf};

use chatoms_domain::{ProjectId, TaskId};
use chatoms_ports::{
    error::{CategorizedFailure, FailureCategory},
    path::ResolvedAppPaths,
    permissions::PermissionStatus,
};
use thiserror::Error;
use tracing::{Dispatch, Level, dispatcher, event};
use tracing_appender::{
    non_blocking::{NonBlocking, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};

use crate::redaction::{RedactedText, RedactionError, SecretRedactor};

pub const LOG_FILE_PREFIX: &str = "chatoms.log";
pub const LOG_ROTATION: Rotation = Rotation::DAILY;
const MAX_EVENT_CODE_LENGTH: usize = 64;
const MAX_FIELD_NAME_LENGTH: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedLogDirectory(PathBuf);

impl ValidatedLogDirectory {
    pub fn from_secure_paths(
        paths: &ResolvedAppPaths,
        permission_status: PermissionStatus,
    ) -> Result<Self, LoggingError> {
        if !permission_status.permits_persistent_storage() {
            return Err(LoggingError::InsecureDirectory);
        }
        if !paths.app_root.is_absolute()
            || !paths.logs_dir.is_absolute()
            || !paths.logs_dir.starts_with(&paths.app_root)
        {
            return Err(LoggingError::InvalidConfiguration);
        }
        Ok(Self(paths.logs_dir.clone()))
    }

    fn as_path(&self) -> &std::path::Path {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggingConfig {
    directory: ValidatedLogDirectory,
    max_level: LogLevel,
}

impl LoggingConfig {
    #[must_use]
    pub const fn new(directory: ValidatedLogDirectory, max_level: LogLevel) -> Self {
        Self {
            directory,
            max_level,
        }
    }

    #[must_use]
    pub const fn max_level(&self) -> LogLevel {
        self.max_level
    }

    #[must_use]
    pub const fn rotation(&self) -> Rotation {
        LOG_ROTATION
    }

    #[must_use]
    pub const fn file_prefix(&self) -> &'static str {
        LOG_FILE_PREFIX
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    const fn tracing_level(self) -> Level {
        match self {
            Self::Error => Level::ERROR,
            Self::Warn => Level::WARN,
            Self::Info => Level::INFO,
            Self::Debug => Level::DEBUG,
            Self::Trace => Level::TRACE,
        }
    }
}

#[derive(Debug)]
pub struct LoggingGuard {
    _guard: WorkerGuard,
}

#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("logging configuration is invalid")]
    InvalidConfiguration,
    #[error("secure logging directory is unavailable")]
    InsecureDirectory,
    #[error("local log appender could not be initialized")]
    AppenderInitialization(#[source] tracing_appender::rolling::InitError),
    #[error("a global tracing subscriber is already installed")]
    SubscriberAlreadyInitialized(#[source] tracing::dispatcher::SetGlobalDefaultError),
    #[error("log event metadata is invalid")]
    InvalidEventMetadata,
    #[error("log content could not be redacted safely")]
    Redaction(#[from] RedactionError),
}

impl CategorizedFailure for LoggingError {
    fn category(&self) -> FailureCategory {
        FailureCategory::LoggingFailure
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeLogMessage(RedactedText);

impl SafeLogMessage {
    pub fn new(redactor: &SecretRedactor, raw: &str) -> Result<Self, LoggingError> {
        let report = redactor.redact_text(raw);
        let validated = redactor.validate_redacted(report.text.as_str())?;
        Ok(Self(validated))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for SafeLogMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeLogField {
    name: String,
    value: RedactedText,
}

impl SafeLogField {
    pub fn new(
        redactor: &SecretRedactor,
        name: &str,
        raw_value: &str,
    ) -> Result<Self, LoggingError> {
        if !valid_field_name(name) {
            return Err(LoggingError::InvalidEventMetadata);
        }
        let report = redactor.redact_field(name, raw_value);
        let value = redactor.validate_redacted(report.text.as_str())?;
        Ok(Self {
            name: name.to_owned(),
            value,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn value(&self) -> &str {
        self.value.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeLogEvent {
    event_code: String,
    level: LogLevel,
    message: SafeLogMessage,
    task_id: Option<TaskId>,
    project_id: Option<ProjectId>,
}

macro_rules! emit_at {
    ($level:expr, $event:expr, $task_id:expr, $project_id:expr) => {
        event!(
            target: "chatoms",
            $level,
            event_code = %$event.event_code,
            message = %$event.message,
            task_id = ?$task_id,
            project_id = ?$project_id,
        )
    };
}

impl SafeLogEvent {
    pub fn new(
        event_code: &str,
        level: LogLevel,
        message: SafeLogMessage,
    ) -> Result<Self, LoggingError> {
        if !valid_event_code(event_code) {
            return Err(LoggingError::InvalidEventMetadata);
        }
        Ok(Self {
            event_code: event_code.to_owned(),
            level,
            message,
            task_id: None,
            project_id: None,
        })
    }

    #[must_use]
    pub const fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    #[must_use]
    pub const fn with_project_id(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self
    }

    pub fn emit(&self) {
        let task_id = self.task_id.map(|id| id.to_string());
        let project_id = self.project_id.map(|id| id.to_string());
        match self.level {
            LogLevel::Error => emit_at!(Level::ERROR, self, task_id, project_id),
            LogLevel::Warn => emit_at!(Level::WARN, self, task_id, project_id),
            LogLevel::Info => emit_at!(Level::INFO, self, task_id, project_id),
            LogLevel::Debug => emit_at!(Level::DEBUG, self, task_id, project_id),
            LogLevel::Trace => emit_at!(Level::TRACE, self, task_id, project_id),
        }
    }
}

pub fn build_file_logging(
    config: &LoggingConfig,
) -> Result<(Dispatch, LoggingGuard), LoggingError> {
    let appender = RollingFileAppender::builder()
        .rotation(config.rotation())
        .filename_prefix(config.file_prefix())
        .build(config.directory.as_path())
        .map_err(LoggingError::AppenderInitialization)?;
    Ok(build_dispatch(appender, config.max_level()))
}

pub fn initialize_logging(config: &LoggingConfig) -> Result<LoggingGuard, LoggingError> {
    let (dispatch, guard) = build_file_logging(config)?;
    install_global(dispatch)?;
    Ok(guard)
}

pub fn install_global(dispatch: Dispatch) -> Result<(), LoggingError> {
    dispatcher::set_global_default(dispatch).map_err(LoggingError::SubscriberAlreadyInitialized)
}

#[doc(hidden)]
pub fn build_scoped_test_logging<W>(writer: W, max_level: LogLevel) -> (Dispatch, LoggingGuard)
where
    W: std::io::Write + Send + 'static,
{
    build_dispatch(writer, max_level)
}

fn build_dispatch<W>(writer: W, max_level: LogLevel) -> (Dispatch, LoggingGuard)
where
    W: std::io::Write + Send + 'static,
{
    let (non_blocking, guard) = tracing_appender::non_blocking(writer);
    (
        dispatch_for(non_blocking, max_level),
        LoggingGuard { _guard: guard },
    )
}

fn dispatch_for(writer: NonBlocking, max_level: LogLevel) -> Dispatch {
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_current_span(false)
        .with_span_list(false)
        .with_max_level(max_level.tracing_level())
        .with_writer(writer)
        .finish();
    Dispatch::new(subscriber)
}

fn valid_event_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_EVENT_CODE_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_field_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FIELD_NAME_LENGTH
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
}
