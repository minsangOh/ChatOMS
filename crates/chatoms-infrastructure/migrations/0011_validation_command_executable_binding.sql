-- Widens task_validation_command_approvals (introduced in the immediately
-- preceding migration, 0010) to also bind the approved logical `executable`
-- name to one specific file: its approved absolute path and Windows stable
-- NTFS object identity (volume serial + file ID), plus the same identity
-- for its containing directory (the future controlled-PATH value). No
-- shipped UI ever wrote to this table, so any pre-existing row is dev/test
-- scratch data, not real user trust state; existing rows are dropped rather
-- than backfilled, since fabricating identity values for them would
-- misrepresent what was actually verified.
CREATE TABLE task_validation_command_approvals_v11 (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    command_kind TEXT NOT NULL CHECK (
        command_kind IN ('Format', 'Lint', 'Typecheck', 'Test', 'Build')
    ),
    executable TEXT NOT NULL CHECK (length(executable) BETWEEN 1 AND 256),
    arguments_json TEXT NOT NULL CHECK (length(arguments_json) BETWEEN 2 AND 4000),
    worktree_scope TEXT NOT NULL CHECK (worktree_scope = 'TaskWorktree'),
    approved_executable_path TEXT NOT NULL CHECK (
        length(approved_executable_path) BETWEEN 1 AND 4096
    ),
    executable_volume_serial_hex TEXT NOT NULL CHECK (
        length(executable_volume_serial_hex) = 16
        AND executable_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
    ),
    executable_file_id_hex TEXT NOT NULL CHECK (
        length(executable_file_id_hex) = 32
        AND executable_file_id_hex NOT GLOB '*[^0-9a-f]*'
    ),
    tool_directory_path TEXT NOT NULL CHECK (
        length(tool_directory_path) BETWEEN 1 AND 4096
    ),
    tool_directory_volume_serial_hex TEXT NOT NULL CHECK (
        length(tool_directory_volume_serial_hex) = 16
        AND tool_directory_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
    ),
    tool_directory_file_id_hex TEXT NOT NULL CHECK (
        length(tool_directory_file_id_hex) = 32
        AND tool_directory_file_id_hex NOT GLOB '*[^0-9a-f]*'
    ),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, command_kind),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

DROP TRIGGER task_validation_command_approvals_binding_insert;
DROP TRIGGER task_validation_command_approvals_immutable_update;
DROP TRIGGER task_validation_command_approvals_immutable_delete;
DROP INDEX task_validation_command_approvals_task_id_idx;
DROP TABLE task_validation_command_approvals;

ALTER TABLE task_validation_command_approvals_v11
    RENAME TO task_validation_command_approvals;

CREATE TRIGGER task_validation_command_approvals_binding_insert
BEFORE INSERT ON task_validation_command_approvals
WHEN NOT EXISTS (
    SELECT 1 FROM tasks
    WHERE id = NEW.task_id AND version = NEW.approved_task_version
)
BEGIN
    SELECT RAISE(ABORT, 'validation command approval task version binding mismatch');
END;

CREATE TRIGGER task_validation_command_approvals_immutable_update
BEFORE UPDATE ON task_validation_command_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_validation_command_approvals is immutable');
END;

CREATE TRIGGER task_validation_command_approvals_immutable_delete
BEFORE DELETE ON task_validation_command_approvals
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_validation_command_approvals is immutable');
END;

CREATE INDEX task_validation_command_approvals_task_id_idx
    ON task_validation_command_approvals (task_id);
