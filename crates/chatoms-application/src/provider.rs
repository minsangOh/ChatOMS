use std::path::Path;

use chatoms_domain::{AppProfileId, ProviderBindingId};
use chatoms_ports::{
    TimeProvider,
    provider::ProviderKind,
    repository::{AppProfileRecord, FoundationRepository, ProviderBindingRecord},
};

use crate::error::ApplicationError;

const DEFAULT_PROFILE_NAME: &str = "Default";
const CLAUDE_BINDING_DISPLAY_NAME: &str = "Claude Code";

pub struct ProviderConfigService<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> ProviderConfigService<'a, R, T>
where
    R: FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    pub fn ensure_default_claude_binding(
        &mut self,
    ) -> Result<ProviderBindingRecord, ApplicationError> {
        let now = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        let profile_id = AppProfileId::new().to_string();
        let binding_id = ProviderBindingId::new().to_string();
        let profile = AppProfileRecord {
            id: profile_id.clone(),
            name: DEFAULT_PROFILE_NAME.to_owned(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        let binding = ProviderBindingRecord {
            id: binding_id,
            app_profile_id: profile_id,
            provider_kind: ProviderKind::Claude,
            display_name: CLAUDE_BINDING_DISPLAY_NAME.to_owned(),
            executable_path: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.repository
            .ensure_default_profile_and_claude_binding(&profile, &binding)
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn get_claude_binding(
        &mut self,
    ) -> Result<Option<ProviderBindingRecord>, ApplicationError> {
        self.repository
            .get_claude_binding(DEFAULT_PROFILE_NAME)
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn set_claude_executable_path(&mut self, path: &str) -> Result<(), ApplicationError> {
        validate_absolute_path(path)?;
        let binding = self.ensure_default_claude_binding()?;
        let now = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        self.repository
            .update_claude_executable_path(&binding.id, Some(path), now)
            .map_err(|error| ApplicationError::from_categorized(&error))
    }

    pub fn clear_claude_executable_path(&mut self) -> Result<(), ApplicationError> {
        let binding = self.ensure_default_claude_binding()?;
        let now = self
            .time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))?;
        self.repository
            .update_claude_executable_path(&binding.id, None, now)
            .map_err(|error| ApplicationError::from_categorized(&error))
    }
}

fn validate_absolute_path(path: &str) -> Result<(), ApplicationError> {
    if path.is_empty() || !Path::new(path).is_absolute() {
        return Err(ApplicationError::from_failure(
            chatoms_ports::error::FailureCategory::InvalidInput,
            chatoms_ports::error::FailureSeverity::Warning,
            chatoms_ports::error::RetryDisposition::AfterUserAction,
        ));
    }
    Ok(())
}
