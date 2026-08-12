use chatoms_application::{error::ApplicationErrorCode, provider::ProviderConfigService};
use chatoms_ports::{
    TimeProvider,
    error::PortFailure,
    provider::ProviderKind,
    repository::{
        AppProfileRecord, FoundationRepository, ProviderBindingRecord, RepositoryError,
        RepositoryErrorCode,
    },
};

struct StubTime {
    value: i64,
}

impl TimeProvider for StubTime {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        Ok(self.value)
    }
}

struct StubRepository {
    ensure_result: Option<Result<ProviderBindingRecord, RepositoryError>>,
    get_result: Option<Result<Option<ProviderBindingRecord>, RepositoryError>>,
    update_result: Option<Result<(), RepositoryError>>,
    last_update_path: Option<Option<String>>,
}

impl StubRepository {
    fn succeeding() -> Self {
        let binding = ProviderBindingRecord {
            id: "binding-1".to_owned(),
            app_profile_id: "profile-1".to_owned(),
            provider_kind: ProviderKind::Claude,
            display_name: "Claude Code".to_owned(),
            executable_path: None,
            created_at_ms: 100,
            updated_at_ms: 100,
        };
        Self {
            ensure_result: Some(Ok(binding)),
            get_result: None,
            update_result: Some(Ok(())),
            last_update_path: None,
        }
    }

    fn failing(code: RepositoryErrorCode) -> Self {
        Self {
            ensure_result: Some(Err(RepositoryError::new(code))),
            get_result: None,
            update_result: None,
            last_update_path: None,
        }
    }
}

impl FoundationRepository for StubRepository {
    fn create_task(
        &mut self,
        _task: &chatoms_domain::Task,
        _initial_transition: &chatoms_domain::TaskStateTransition,
        _lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn get_task(
        &mut self,
        _task_id: chatoms_domain::TaskId,
    ) -> Result<Option<chatoms_domain::Task>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn save_transition(
        &mut self,
        _expected_version: u64,
        _task: &chatoms_domain::Task,
        _transition: &chatoms_domain::TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn save_recovery_target(
        &mut self,
        _expected_version: u64,
        _task: &chatoms_domain::Task,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn terminate_task(
        &mut self,
        _expected_version: u64,
        _task: &chatoms_domain::Task,
        _transition: &chatoms_domain::TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn list_task_transitions(
        &mut self,
        _task_id: chatoms_domain::TaskId,
    ) -> Result<Vec<chatoms_domain::TaskStateTransition>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn list_projects(
        &mut self,
    ) -> Result<Vec<chatoms_ports::repository::ProjectSummary>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn active_lease(
        &mut self,
    ) -> Result<Option<chatoms_ports::repository::ActiveLease>, RepositoryError> {
        Err(RepositoryError::new(RepositoryErrorCode::OperationFailed))
    }

    fn ensure_default_profile_and_claude_binding(
        &mut self,
        _profile: &AppProfileRecord,
        _binding: &ProviderBindingRecord,
    ) -> Result<ProviderBindingRecord, RepositoryError> {
        self.ensure_result
            .take()
            .unwrap_or_else(|| Err(RepositoryError::new(RepositoryErrorCode::OperationFailed)))
    }

    fn get_claude_binding(
        &mut self,
        _profile_name: &str,
    ) -> Result<Option<ProviderBindingRecord>, RepositoryError> {
        self.get_result
            .take()
            .unwrap_or_else(|| Err(RepositoryError::new(RepositoryErrorCode::OperationFailed)))
    }

    fn update_claude_executable_path(
        &mut self,
        _binding_id: &str,
        executable_path: Option<&str>,
        _updated_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.last_update_path = Some(executable_path.map(str::to_owned));
        self.update_result
            .take()
            .unwrap_or_else(|| Err(RepositoryError::new(RepositoryErrorCode::OperationFailed)))
    }
}

#[test]
fn ensure_default_claude_binding_uses_correct_constants() {
    let mut repository = StubRepository::succeeding();
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    let result = service.ensure_default_claude_binding().expect("ensure");
    assert_eq!(result.display_name, "Claude Code");
    assert_eq!(result.provider_kind, ProviderKind::Claude);
}

#[test]
fn set_claude_executable_path_rejects_empty_string() {
    let mut repository = StubRepository::succeeding();
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    let error = service
        .set_claude_executable_path("")
        .expect_err("empty path must fail");
    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
}

#[test]
fn set_claude_executable_path_rejects_relative_path() {
    let mut repository = StubRepository::succeeding();
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    let error = service
        .set_claude_executable_path("relative/path/claude.exe")
        .expect_err("relative path must fail");
    assert_eq!(error.code(), ApplicationErrorCode::InvalidInput);
}

#[test]
fn set_claude_executable_path_accepts_absolute_path() {
    let mut repository = StubRepository::succeeding();
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    service
        .set_claude_executable_path("C:\\Users\\test\\claude.exe")
        .expect("absolute path must succeed");
    assert_eq!(
        repository.last_update_path,
        Some(Some("C:\\Users\\test\\claude.exe".to_owned()))
    );
}

#[test]
fn clear_claude_executable_path_passes_none() {
    let binding = ProviderBindingRecord {
        id: "binding-1".to_owned(),
        app_profile_id: "profile-1".to_owned(),
        provider_kind: ProviderKind::Claude,
        display_name: "Claude Code".to_owned(),
        executable_path: Some("C:/existing.exe".to_owned()),
        created_at_ms: 100,
        updated_at_ms: 100,
    };
    let mut repository = StubRepository {
        ensure_result: Some(Ok(binding)),
        get_result: None,
        update_result: Some(Ok(())),
        last_update_path: None,
    };
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    service
        .clear_claude_executable_path()
        .expect("clear must succeed");
    assert_eq!(repository.last_update_path, Some(None));
}

#[test]
fn get_claude_binding_returns_none_when_absent() {
    let mut repository = StubRepository {
        ensure_result: None,
        get_result: Some(Ok(None)),
        update_result: None,
        last_update_path: None,
    };
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    let result = service.get_claude_binding().expect("get binding");
    assert!(result.is_none());
}

#[test]
fn repository_error_maps_to_application_error() {
    let mut repository = StubRepository::failing(RepositoryErrorCode::DatabaseUnavailable);
    let mut time = StubTime { value: 500 };
    let mut service = ProviderConfigService::new(&mut repository, &mut time);
    let error = service
        .ensure_default_claude_binding()
        .expect_err("database unavailable must fail");
    assert_eq!(error.code(), ApplicationErrorCode::StorageUnavailable);
}
