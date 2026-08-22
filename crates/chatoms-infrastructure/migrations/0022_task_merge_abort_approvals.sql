-- Immutable, content-free approval that a user explicitly approved
-- aborting one task's in-progress `MergeConflict` merge. This is a separate
-- approval axis from task_manual_merge_resolution_confirmations (0021):
-- that confirmation approves *continuing* a specific staged resolution and
-- binds to its resolution digest, while this approval discards the merge
-- entirely -- a still-unresolved staged index is just as valid an abort
-- target as an already-resolved one, so this table deliberately does not
-- bind to any resolution digest. Not foreign-keyed to task_diff_approvals,
-- task_provider_consents, context_package_manifests,
-- task_high_risk_approvals, or task_manual_merge_resolution_confirmations --
-- a task may need any combination of these entirely independently.
--
-- Unlike 0021, there is no digest column: approval identity is
-- (task_id, merge_conflict_task_version) alone, so at most one abort
-- approval can ever exist per MergeConflict occurrence. task_commit_hex
-- must equal merge_head_hex for the same reason as 0021: MERGE_HEAD only
-- ever points at the task commit being merged, so any other value is a
-- corrupted or hand-edited row, not a legitimate approval.
CREATE TABLE task_merge_abort_approvals (
    task_id TEXT NOT NULL,
    merge_conflict_task_version INTEGER NOT NULL CHECK (
        merge_conflict_task_version >= 0
    ),
    source_approval_task_version INTEGER NOT NULL CHECK (
        source_approval_task_version >= 0
    ),
    base_commit_hex TEXT NOT NULL CHECK (
        (length(base_commit_hex) = 40 OR length(base_commit_hex) = 64)
        AND base_commit_hex NOT GLOB '*[^0-9a-f]*'
    ),
    task_commit_hex TEXT NOT NULL CHECK (
        (length(task_commit_hex) = 40 OR length(task_commit_hex) = 64)
        AND task_commit_hex NOT GLOB '*[^0-9a-f]*'
    ),
    merge_head_hex TEXT NOT NULL CHECK (
        (length(merge_head_hex) = 40 OR length(merge_head_hex) = 64)
        AND merge_head_hex NOT GLOB '*[^0-9a-f]*'
    ),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, merge_conflict_task_version),
    CHECK (task_commit_hex = merge_head_hex),
    CHECK (base_commit_hex != task_commit_hex),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_merge_abort_approvals_binding_insert
BEFORE INSERT ON task_merge_abort_approvals
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id
      AND version = NEW.merge_conflict_task_version
      AND state = 'MergeConflict'
)
BEGIN
    SELECT RAISE(ABORT, 'merge abort approval task version binding mismatch');
END;

CREATE TRIGGER task_merge_abort_approvals_immutable_update
BEFORE UPDATE ON task_merge_abort_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_merge_abort_approvals is immutable');
END;

CREATE TRIGGER task_merge_abort_approvals_immutable_delete
BEFORE DELETE ON task_merge_abort_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_merge_abort_approvals is immutable');
END;

CREATE INDEX task_merge_abort_approvals_task_id_idx
    ON task_merge_abort_approvals (task_id);
