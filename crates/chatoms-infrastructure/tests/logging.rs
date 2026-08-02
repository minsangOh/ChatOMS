use std::{
    fs,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use chatoms_domain::{ProjectId, TaskId};
use chatoms_infrastructure::{
    logging::{
        LOG_FILE_PREFIX, LOG_ROTATION, LogLevel, LoggingConfig, LoggingError, SafeLogEvent,
        SafeLogField, SafeLogMessage, ValidatedLogDirectory, build_file_logging,
        build_scoped_test_logging, install_global,
    },
    redaction::SecretRedactor,
};
use chatoms_ports::{path::ResolvedAppPaths, permissions::PermissionStatus};
use tempfile::TempDir;
use tracing::dispatcher;
use tracing_appender::rolling::Rotation;

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("buffer lock").clone()).expect("UTF-8 log")
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("buffer poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn paths(temp: &TempDir) -> ResolvedAppPaths {
    let app_root = temp.path().join("ChatOMS");
    ResolvedAppPaths {
        data_dir: app_root.join("data"),
        database_path: app_root.join("data/chatoms.sqlite3"),
        logs_dir: app_root.join("logs"),
        artifacts_dir: app_root.join("artifacts"),
        temp_dir: app_root.join("temp"),
        app_root,
    }
}

fn safe_event(redactor: &SecretRedactor, raw: &str) -> SafeLogEvent {
    SafeLogEvent::new(
        "SYSTEM_HEALTH_CHECKED",
        LogLevel::Info,
        SafeLogMessage::new(redactor, raw).expect("message redaction"),
    )
    .expect("event metadata")
}

#[test]
fn structured_log_contains_safe_fields_ids_level_target_and_no_ansi() {
    let redactor = SecretRedactor::new().expect("redactor");
    let task_id = TaskId::new();
    let project_id = ProjectId::new();
    let buffer = SharedBuffer::default();
    let (dispatch, guard) = build_scoped_test_logging(buffer.clone(), LogLevel::Trace);

    dispatcher::with_default(&dispatch, || {
        safe_event(
            &redactor,
            "Authorization: Bearer auth-secret password=token-secret",
        )
        .with_task_id(task_id)
        .with_project_id(project_id)
        .emit();
    });
    drop(guard);

    let output = buffer.text();
    for expected in [
        "SYSTEM_HEALTH_CHECKED",
        "INFO",
        "chatoms",
        &task_id.to_string(),
        &project_id.to_string(),
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    assert!(!output.contains("auth-secret"));
    assert!(!output.contains("token-secret"));
    assert!(!output.contains("\u{1b}["));
}

#[test]
fn private_key_and_sensitive_field_values_are_redacted_before_logging() {
    let redactor = SecretRedactor::new().expect("redactor");
    let message = SafeLogMessage::new(
        &redactor,
        "-----BEGIN PRIVATE KEY-----\nkey-secret\n-----END PRIVATE KEY-----",
    )
    .expect("message");
    assert!(!message.as_str().contains("key-secret"));

    let field = SafeLogField::new(&redactor, "access_token", "opaque-secret").expect("field");
    assert_eq!(field.name(), "access_token");
    assert!(!field.value().contains("opaque-secret"));
}

#[test]
fn file_logging_uses_secure_directory_daily_rotation_and_flushes_utf8() {
    let temp = TempDir::new().expect("temp directory");
    let paths = paths(&temp);
    fs::create_dir_all(&paths.logs_dir).expect("logs directory");
    let directory = ValidatedLogDirectory::from_secure_paths(&paths, PermissionStatus::Secure)
        .expect("secure path token");
    let config = LoggingConfig::new(directory, LogLevel::Info);
    assert_eq!(config.rotation(), Rotation::DAILY);
    assert_eq!(LOG_ROTATION, Rotation::DAILY);
    assert_eq!(config.file_prefix(), LOG_FILE_PREFIX);

    let (dispatch, guard) = build_file_logging(&config).expect("file logger");
    let redactor = SecretRedactor::new().expect("redactor");
    dispatcher::with_default(&dispatch, || {
        safe_event(&redactor, "안전한 진단 메시지").emit();
    });
    drop(guard);

    let files = fs::read_dir(&paths.logs_dir)
        .expect("read logs")
        .collect::<Result<Vec<_>, _>>()
        .expect("log entries");
    assert_eq!(files.len(), 1);
    assert!(
        files[0]
            .file_name()
            .to_string_lossy()
            .starts_with(LOG_FILE_PREFIX)
    );
    let output = fs::read_to_string(files[0].path()).expect("log is flushed UTF-8");
    assert!(output.contains("안전한 진단 메시지"));
}

#[test]
fn insecure_or_invalid_paths_are_rejected_before_file_creation() {
    let temp = TempDir::new().expect("temp directory");
    let paths = paths(&temp);
    assert!(matches!(
        ValidatedLogDirectory::from_secure_paths(&paths, PermissionStatus::Insecure),
        Err(LoggingError::InsecureDirectory)
    ));

    let mut outside = paths.clone();
    outside.logs_dir = temp.path().join("outside");
    assert!(matches!(
        ValidatedLogDirectory::from_secure_paths(&outside, PermissionStatus::Secure),
        Err(LoggingError::InvalidConfiguration)
    ));
    assert!(!paths.logs_dir.exists());
    assert!(!outside.logs_dir.exists());
}

#[test]
fn duplicate_global_subscriber_is_a_typed_safe_error() {
    let first = build_scoped_test_logging(io::sink(), LogLevel::Info).0;
    install_global(first).expect("first subscriber");
    let second = build_scoped_test_logging(io::sink(), LogLevel::Info).0;
    let error = install_global(second).expect_err("duplicate subscriber must fail");
    assert!(matches!(
        error,
        LoggingError::SubscriberAlreadyInitialized(_)
    ));
    let displayed = error.to_string();
    assert!(!displayed.contains("C:\\"));
    assert!(!displayed.contains("secret"));
}

#[test]
fn invalid_event_and_field_metadata_are_rejected() {
    let redactor = SecretRedactor::new().expect("redactor");
    let message = SafeLogMessage::new(&redactor, "safe").expect("message");
    assert!(matches!(
        SafeLogEvent::new("bad-event", LogLevel::Info, message),
        Err(LoggingError::InvalidEventMetadata)
    ));
    assert!(matches!(
        SafeLogField::new(&redactor, "Bad Field", "safe"),
        Err(LoggingError::InvalidEventMetadata)
    ));
}
