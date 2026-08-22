-- Immutable, content-free confirmation that a user reviewed and approved
-- the exact staged index a manual `MergeConflict` resolution left in the
-- original checkout. This is a separate approval axis from
-- task_diff_approvals (0019): that table binds to the task diff reviewed
-- *before* Merging ever starts, while this table only ever exists once a
-- real MergeConflict required a human decision, and it binds to the
-- staged index of the original checkout mid-merge, not the task worktree's
-- diff. Not foreign-keyed to task_diff_approvals, task_provider_consents,
-- context_package_manifests, or task_high_risk_approvals -- a task may need
-- any combination of these entirely independently.
--
-- Unlike task_high_risk_approvals's closed vocabulary, resolution_digest_hex
-- is a SHA-256 digest (chatoms.manual-merge-resolution.v1 envelope) over
-- task/project identity, both task-version components, base/task branch,
-- base/task/MERGE_HEAD commit, and every staged (mode, object id, path)
-- triple in the original checkout's index -- never the raw path or file
-- content itself, which this table never stores. task_commit_hex must equal
-- merge_head_hex: MERGE_HEAD only ever points at the task commit being
-- merged, so any other value is a corrupted or hand-edited row, not a
-- legitimate confirmation.
CREATE TABLE task_manual_merge_resolution_confirmations (
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
    resolution_digest_hex TEXT NOT NULL CHECK (
        length(resolution_digest_hex) = 64
        AND resolution_digest_hex NOT GLOB '*[^0-9a-f]*'
    ),
    confirmed_at_ms INTEGER NOT NULL CHECK (confirmed_at_ms >= 0),
    PRIMARY KEY (task_id, merge_conflict_task_version, resolution_digest_hex),
    CHECK (task_commit_hex = merge_head_hex),
    CHECK (base_commit_hex != task_commit_hex),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_manual_merge_resolution_confirmations_binding_insert
BEFORE INSERT ON task_manual_merge_resolution_confirmations
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id
      AND version = NEW.merge_conflict_task_version
      AND state = 'MergeConflict'
)
BEGIN
    SELECT RAISE(ABORT, 'manual merge resolution confirmation task version binding mismatch');
END;

CREATE TRIGGER task_manual_merge_resolution_confirmations_immutable_update
BEFORE UPDATE ON task_manual_merge_resolution_confirmations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_manual_merge_resolution_confirmations is immutable');
END;

CREATE TRIGGER task_manual_merge_resolution_confirmations_immutable_delete
BEFORE DELETE ON task_manual_merge_resolution_confirmations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_manual_merge_resolution_confirmations is immutable');
END;

CREATE INDEX task_manual_merge_resolution_confirmations_task_id_idx
    ON task_manual_merge_resolution_confirmations (task_id);
