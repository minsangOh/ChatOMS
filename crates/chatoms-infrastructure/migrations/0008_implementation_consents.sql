CREATE TABLE task_provider_consents_v8 (
    task_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider = 'Claude'),
    work_kind TEXT NOT NULL CHECK (work_kind IN ('Planning', 'Implementation')),
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    consented_at_ms INTEGER NOT NULL CHECK (consented_at_ms >= 0),
    PRIMARY KEY (task_id, provider, work_kind, approved_task_version),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

INSERT INTO task_provider_consents_v8 (
    task_id, provider, work_kind, approved_task_version, consented_at_ms
)
SELECT task_id, provider, work_kind, approved_task_version, consented_at_ms
FROM task_provider_consents;

DROP TRIGGER task_provider_consents_binding_insert;
DROP TRIGGER task_provider_consents_immutable_update;
DROP TRIGGER task_provider_consents_immutable_delete;
DROP INDEX task_provider_consents_task_id_idx;
DROP TABLE task_provider_consents;

ALTER TABLE task_provider_consents_v8 RENAME TO task_provider_consents;

CREATE TRIGGER task_provider_consents_binding_insert
BEFORE INSERT ON task_provider_consents
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id AND version = NEW.approved_task_version
)
BEGIN
    SELECT RAISE(ABORT, 'provider consent task version binding mismatch');
END;

CREATE TRIGGER task_provider_consents_immutable_update
BEFORE UPDATE ON task_provider_consents
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_provider_consents is immutable');
END;

CREATE TRIGGER task_provider_consents_immutable_delete
BEFORE DELETE ON task_provider_consents
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_provider_consents is immutable');
END;

CREATE INDEX task_provider_consents_task_id_idx ON task_provider_consents (task_id);
