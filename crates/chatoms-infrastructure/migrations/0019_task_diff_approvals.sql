-- Immutable, content-free approval binding a specific task version to the
-- exact SHA-256 content hash of the worktree diff the user reviewed and
-- approved (chatoms_ports::diff::DiffContentHash) -- never the raw diff
-- text itself, which is never persisted anywhere. This is a separate axis
-- from task_high_risk_approvals (0018), task_provider_consents (0006/0016)
-- and context_package_manifests (0017): none of those answer "did the user
-- approve this exact reviewed diff at this task version", so this table is
-- not foreign-keyed to any of them.
--
-- Unlike task_high_risk_approvals's closed 13-category vocabulary, the diff
-- this approval covers has no prior commit to bind to at approval time (the
-- single work commit is only created once Merging starts -- see
-- docs/DECISIONS.md's "병합 이력" entry), so the content hash is the only
-- content-free way to prove "the user approved *this exact* diff, not a
-- different one at the same task version". A future Merging Unit is
-- responsible for recomputing the current diff hash immediately before
-- merging and treating any mismatch as fail-closed (RecoveryRequired) --
-- this table only stores and reads back the approval reference.
CREATE TABLE task_diff_approvals (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    diff_content_hash_hex TEXT NOT NULL CHECK (
        length(diff_content_hash_hex) = 64
        AND diff_content_hash_hex NOT GLOB '*[^0-9a-f]*'
    ),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, diff_content_hash_hex),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_diff_approvals_binding_insert
BEFORE INSERT ON task_diff_approvals
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id AND version = NEW.approved_task_version
)
BEGIN
    SELECT RAISE(ABORT, 'diff approval task version binding mismatch');
END;

CREATE TRIGGER task_diff_approvals_immutable_update
BEFORE UPDATE ON task_diff_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_diff_approvals is immutable');
END;

CREATE TRIGGER task_diff_approvals_immutable_delete
BEFORE DELETE ON task_diff_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_diff_approvals is immutable');
END;

CREATE INDEX task_diff_approvals_task_id_idx ON task_diff_approvals (task_id);
