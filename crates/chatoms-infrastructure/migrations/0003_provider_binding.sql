ALTER TABLE provider_bindings ADD COLUMN executable_path TEXT NULL;

CREATE UNIQUE INDEX provider_bindings_profile_kind_unique
ON provider_bindings (app_profile_id, provider_kind);

CREATE UNIQUE INDEX app_profiles_name_unique
ON app_profiles (name);

CREATE TRIGGER provider_bindings_executable_path_not_empty
BEFORE INSERT ON provider_bindings
WHEN NEW.executable_path IS NOT NULL AND length(NEW.executable_path) = 0
BEGIN
    SELECT RAISE(ABORT, 'provider_bindings.executable_path must not be empty');
END;

CREATE TRIGGER provider_bindings_executable_path_not_empty_update
BEFORE UPDATE OF executable_path ON provider_bindings
WHEN NEW.executable_path IS NOT NULL AND length(NEW.executable_path) = 0
BEGIN
    SELECT RAISE(ABORT, 'provider_bindings.executable_path must not be empty');
END;

CREATE TRIGGER provider_bindings_executable_path_claude_only
BEFORE INSERT ON provider_bindings
WHEN NEW.executable_path IS NOT NULL AND NEW.provider_kind != 'Claude'
BEGIN
    SELECT RAISE(ABORT, 'executable_path is only allowed for Claude bindings');
END;

CREATE TRIGGER provider_bindings_executable_path_claude_only_update
BEFORE UPDATE OF executable_path ON provider_bindings
WHEN NEW.executable_path IS NOT NULL AND NEW.provider_kind != 'Claude'
BEGIN
    SELECT RAISE(ABORT, 'executable_path is only allowed for Claude bindings');
END;

CREATE TRIGGER provider_bindings_identity_immutable
BEFORE UPDATE OF id, app_profile_id, provider_kind, created_at_ms ON provider_bindings
WHEN NEW.id IS NOT OLD.id
    OR NEW.app_profile_id IS NOT OLD.app_profile_id
    OR NEW.provider_kind IS NOT OLD.provider_kind
    OR NEW.created_at_ms IS NOT OLD.created_at_ms
BEGIN
    SELECT RAISE(ABORT, 'provider_bindings identity columns are immutable');
END;
