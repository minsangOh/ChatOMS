-- Widens task_provider_consents (last widened in 0014 to add the Review
-- work_kind) to bind a fifth identity component: the data scope a consent
-- actually approves for transmission. Every Phase 4 Planning/Implementation/
-- Review consent row already in this table was granted under one fixed,
-- code-defined payload shape (TaskBrief's three fields, plus the prior plan
-- text for Implementation or the current diff for Review) — that shape is
-- 'LegacyPhase4'. Unlike 0011 (whose pre-existing rows were dev/test
-- scratch data with no shipped writer), every existing row here was written
-- by shipped Unit 4b-1/4c-1/4e-2 code, so this migration backfills
-- 'LegacyPhase4' for all of them rather than dropping anything.
-- 'ContextPackageV1' is reserved vocabulary for a future Context Package
-- manifest Unit; nothing writes it yet.
CREATE TABLE task_provider_consents_v16 (
    task_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider = 'Claude'),
    work_kind TEXT NOT NULL CHECK (work_kind IN ('Planning', 'Implementation', 'Review')),
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    data_scope TEXT NOT NULL CHECK (data_scope IN ('LegacyPhase4', 'ContextPackageV1')),
    consented_at_ms INTEGER NOT NULL CHECK (consented_at_ms >= 0),
    PRIMARY KEY (task_id, provider, work_kind, approved_task_version, data_scope),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

INSERT INTO task_provider_consents_v16 (
    task_id, provider, work_kind, approved_task_version, data_scope, consented_at_ms
)
SELECT task_id, provider, work_kind, approved_task_version, 'LegacyPhase4', consented_at_ms
FROM task_provider_consents;

DROP TRIGGER task_provider_consents_binding_insert;
DROP TRIGGER task_provider_consents_immutable_update;
DROP TRIGGER task_provider_consents_immutable_delete;
DROP INDEX task_provider_consents_task_id_idx;
DROP TABLE task_provider_consents;

ALTER TABLE task_provider_consents_v16 RENAME TO task_provider_consents;

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
