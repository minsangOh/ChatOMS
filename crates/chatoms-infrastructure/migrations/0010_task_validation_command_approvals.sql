CREATE TABLE task_validation_command_approvals (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    command_kind TEXT NOT NULL CHECK (
        command_kind IN ('Format', 'Lint', 'Typecheck', 'Test', 'Build')
    ),
    executable TEXT NOT NULL CHECK (length(executable) BETWEEN 1 AND 256),
    arguments_json TEXT NOT NULL CHECK (length(arguments_json) BETWEEN 2 AND 4000),
    worktree_scope TEXT NOT NULL CHECK (worktree_scope = 'TaskWorktree'),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, command_kind),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

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
