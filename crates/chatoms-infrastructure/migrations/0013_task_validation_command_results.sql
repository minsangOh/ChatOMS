-- Append-only storage for validation command execution attempts. Unlike
-- task_planning_results/task_implementation_results (one row per task),
-- Testing can re-enter through AutoFixing/ReviewFixing many times, so a
-- single approved (task_id, approved_task_version, command_kind) may
-- accumulate many attempts. attempt_sequence orders them, starting at 1,
-- and is computed atomically by the repository, never supplied by a
-- caller.
--
-- safe_summary is a masked, size-bounded summary produced by a future
-- orchestration Unit (e.g. via the existing SecretRedactor) -- it is never
-- raw stdout/stderr, and no column here ever stores raw process output,
-- a transcript, provider/session identifiers, an executable path, or an
-- environment path. The approval row this result is bound to already
-- records the command's executable/tool-directory/environment identity
-- immutably; this table adds nothing beyond the attempt's own safe outcome.
CREATE TABLE task_validation_command_results (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    command_kind TEXT NOT NULL CHECK (
        command_kind IN ('Format', 'Lint', 'Typecheck', 'Test', 'Build')
    ),
    attempt_sequence INTEGER NOT NULL CHECK (attempt_sequence >= 1),
    outcome TEXT NOT NULL CHECK (
        outcome IN (
            'Success', 'ExitFailure', 'TimedOut', 'StdoutBoundExceeded',
            'Cancelled', 'Uncertain'
        )
    ),
    exit_code INTEGER NULL,
    safe_summary TEXT NOT NULL CHECK (length(safe_summary) BETWEEN 1 AND 2000),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    completed_at_ms INTEGER NOT NULL CHECK (completed_at_ms >= started_at_ms),
    PRIMARY KEY (task_id, approved_task_version, command_kind, attempt_sequence),
    CHECK (
        (outcome IN ('Success', 'ExitFailure') AND exit_code IS NOT NULL)
        OR (outcome NOT IN ('Success', 'ExitFailure') AND exit_code IS NULL)
    ),
    FOREIGN KEY (task_id, approved_task_version, command_kind)
        REFERENCES task_validation_command_approvals (
            task_id, approved_task_version, command_kind
        )
);

CREATE TRIGGER task_validation_command_results_sequence_insert
BEFORE INSERT ON task_validation_command_results
WHEN NEW.attempt_sequence != (
    SELECT COUNT(*) + 1 FROM task_validation_command_results
    WHERE task_id = NEW.task_id
      AND approved_task_version = NEW.approved_task_version
      AND command_kind = NEW.command_kind
)
BEGIN
    SELECT RAISE(ABORT, 'validation command result attempt sequence mismatch');
END;

CREATE TRIGGER task_validation_command_results_immutable_update
BEFORE UPDATE ON task_validation_command_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_validation_command_results is immutable');
END;

CREATE TRIGGER task_validation_command_results_immutable_delete
BEFORE DELETE ON task_validation_command_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_validation_command_results is immutable');
END;

CREATE INDEX task_validation_command_results_task_id_idx
    ON task_validation_command_results (task_id);
