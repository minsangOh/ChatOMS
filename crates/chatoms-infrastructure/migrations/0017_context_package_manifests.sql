-- Immutable, content-free identity record proving a Context Package v1
-- manifest exists for a specific (task_id, provider, work_kind,
-- approved_task_version, data_scope) provider-transmission consent.
-- Alternative B (docs/DECISIONS.md, "Context Package v1 저장 방식") permanently
-- stores only this immutable reference plus content-free metadata; the
-- actual package body (TaskBrief text, plan/review/validation content, Git
-- diff, file/symbol references, or any other content) is assembled only
-- immediately before a provider call and is never persisted here or
-- anywhere else. This table therefore carries no content field at all.
--
-- A manifest can only ever exist for a ContextPackageV1-scoped consent --
-- LegacyPhase4 has no manifest concept -- so data_scope is pinned to that
-- single value, and the full five-column identity is a foreign key into
-- task_provider_consents (0016_provider_consent_data_scope.sql), whose
-- primary key is exactly this same 5-tuple. This is a 1:1 relationship:
-- a manifest cannot be inserted before its matching consent row exists, and
-- each consent can be covered by at most one manifest.
CREATE TABLE context_package_manifests (
    task_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider = 'Claude'),
    work_kind TEXT NOT NULL CHECK (work_kind IN ('Planning', 'Implementation', 'Review')),
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    data_scope TEXT NOT NULL CHECK (data_scope = 'ContextPackageV1'),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (task_id, provider, work_kind, approved_task_version, data_scope),
    FOREIGN KEY (task_id, provider, work_kind, approved_task_version, data_scope)
        REFERENCES task_provider_consents (
            task_id, provider, work_kind, approved_task_version, data_scope
        )
);

CREATE TRIGGER context_package_manifests_binding_insert
BEFORE INSERT ON context_package_manifests
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id AND version = NEW.approved_task_version
)
BEGIN
    SELECT RAISE(ABORT, 'context package manifest task version binding mismatch');
END;

CREATE TRIGGER context_package_manifests_immutable_update
BEFORE UPDATE ON context_package_manifests
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'context_package_manifests is immutable');
END;

CREATE TRIGGER context_package_manifests_immutable_delete
BEFORE DELETE ON context_package_manifests
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'context_package_manifests is immutable');
END;

CREATE INDEX context_package_manifests_task_id_idx ON context_package_manifests (task_id);
