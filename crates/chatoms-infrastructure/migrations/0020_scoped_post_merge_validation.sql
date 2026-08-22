CREATE TABLE task_validation_command_approvals_v20 (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    execution_scope TEXT NOT NULL CHECK (
        execution_scope IN ('TaskWorktree', 'ProjectRoot')
    ),
    command_kind TEXT NOT NULL CHECK (
        command_kind IN ('Format', 'Lint', 'Typecheck', 'Test', 'Build')
    ),
    executable TEXT NOT NULL CHECK (length(executable) BETWEEN 1 AND 256),
    arguments_json TEXT NOT NULL CHECK (length(arguments_json) BETWEEN 2 AND 4000),
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
    approved_cargo_home_path TEXT NULL CHECK (
        approved_cargo_home_path IS NULL
        OR length(approved_cargo_home_path) BETWEEN 1 AND 4096
    ),
    cargo_home_volume_serial_hex TEXT NULL CHECK (
        cargo_home_volume_serial_hex IS NULL OR (
            length(cargo_home_volume_serial_hex) = 16
            AND cargo_home_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    cargo_home_file_id_hex TEXT NULL CHECK (
        cargo_home_file_id_hex IS NULL OR (
            length(cargo_home_file_id_hex) = 32
            AND cargo_home_file_id_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    approved_rustup_home_path TEXT NULL CHECK (
        approved_rustup_home_path IS NULL
        OR length(approved_rustup_home_path) BETWEEN 1 AND 4096
    ),
    rustup_home_volume_serial_hex TEXT NULL CHECK (
        rustup_home_volume_serial_hex IS NULL OR (
            length(rustup_home_volume_serial_hex) = 16
            AND rustup_home_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    rustup_home_file_id_hex TEXT NULL CHECK (
        rustup_home_file_id_hex IS NULL OR (
            length(rustup_home_file_id_hex) = 32
            AND rustup_home_file_id_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    target_project_id TEXT NULL,
    target_project_identity_revision INTEGER NULL CHECK (
        target_project_identity_revision IS NULL OR target_project_identity_revision >= 1
    ),
    target_root_volume_serial_hex TEXT NULL CHECK (
        target_root_volume_serial_hex IS NULL OR (
            length(target_root_volume_serial_hex) = 16
            AND target_root_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    target_root_file_id_hex TEXT NULL CHECK (
        target_root_file_id_hex IS NULL OR (
            length(target_root_file_id_hex) = 32
            AND target_root_file_id_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, execution_scope, command_kind),
    CHECK (
        (approved_cargo_home_path IS NULL
            AND cargo_home_volume_serial_hex IS NULL
            AND cargo_home_file_id_hex IS NULL)
        OR (approved_cargo_home_path IS NOT NULL
            AND cargo_home_volume_serial_hex IS NOT NULL
            AND cargo_home_file_id_hex IS NOT NULL)
    ),
    CHECK (
        (approved_rustup_home_path IS NULL
            AND rustup_home_volume_serial_hex IS NULL
            AND rustup_home_file_id_hex IS NULL)
        OR (approved_rustup_home_path IS NOT NULL
            AND rustup_home_volume_serial_hex IS NOT NULL
            AND rustup_home_file_id_hex IS NOT NULL)
    ),
    CHECK (
        (execution_scope = 'TaskWorktree'
            AND target_project_id IS NULL
            AND target_project_identity_revision IS NULL
            AND target_root_volume_serial_hex IS NULL
            AND target_root_file_id_hex IS NULL)
        OR (execution_scope = 'ProjectRoot'
            AND target_project_id IS NOT NULL
            AND target_project_identity_revision IS NOT NULL
            AND target_root_volume_serial_hex IS NOT NULL
            AND target_root_file_id_hex IS NOT NULL)
    ),
    FOREIGN KEY (task_id) REFERENCES tasks (id),
    FOREIGN KEY (target_project_id) REFERENCES projects (id)
);

INSERT INTO task_validation_command_approvals_v20 (
    task_id, approved_task_version, execution_scope, command_kind,
    executable, arguments_json, approved_executable_path,
    executable_volume_serial_hex, executable_file_id_hex,
    tool_directory_path, tool_directory_volume_serial_hex,
    tool_directory_file_id_hex, approved_cargo_home_path,
    cargo_home_volume_serial_hex, cargo_home_file_id_hex,
    approved_rustup_home_path, rustup_home_volume_serial_hex,
    rustup_home_file_id_hex, approved_at_ms
)
SELECT task_id, approved_task_version, 'TaskWorktree', command_kind,
       executable, arguments_json, approved_executable_path,
       executable_volume_serial_hex, executable_file_id_hex,
       tool_directory_path, tool_directory_volume_serial_hex,
       tool_directory_file_id_hex, approved_cargo_home_path,
       cargo_home_volume_serial_hex, cargo_home_file_id_hex,
       approved_rustup_home_path, rustup_home_volume_serial_hex,
       rustup_home_file_id_hex, approved_at_ms
FROM task_validation_command_approvals;

CREATE TABLE task_validation_command_results_v20 (
    task_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    execution_scope TEXT NOT NULL CHECK (execution_scope = 'TaskWorktree'),
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
    PRIMARY KEY (
        task_id, approved_task_version, execution_scope, command_kind, attempt_sequence
    ),
    CHECK (
        (outcome IN ('Success', 'ExitFailure') AND exit_code IS NOT NULL)
        OR (outcome NOT IN ('Success', 'ExitFailure') AND exit_code IS NULL)
    ),
    FOREIGN KEY (task_id, approved_task_version, execution_scope, command_kind)
        REFERENCES task_validation_command_approvals_v20 (
            task_id, approved_task_version, execution_scope, command_kind
        )
);

INSERT INTO task_validation_command_results_v20 (
    task_id, approved_task_version, execution_scope, command_kind,
    attempt_sequence, outcome, exit_code, safe_summary,
    started_at_ms, completed_at_ms
)
SELECT task_id, approved_task_version, 'TaskWorktree', command_kind,
       attempt_sequence, outcome, exit_code, safe_summary,
       started_at_ms, completed_at_ms
FROM task_validation_command_results;

DROP TRIGGER task_validation_command_results_sequence_insert;
DROP TRIGGER task_validation_command_results_immutable_update;
DROP TRIGGER task_validation_command_results_immutable_delete;
DROP INDEX task_validation_command_results_task_id_idx;
DROP TABLE task_validation_command_results;

DROP TRIGGER task_validation_command_approvals_binding_insert;
DROP TRIGGER task_validation_command_approvals_immutable_update;
DROP TRIGGER task_validation_command_approvals_immutable_delete;
DROP INDEX task_validation_command_approvals_task_id_idx;
DROP TABLE task_validation_command_approvals;

ALTER TABLE task_validation_command_approvals_v20
    RENAME TO task_validation_command_approvals;
ALTER TABLE task_validation_command_results_v20
    RENAME TO task_validation_command_results;

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

CREATE TRIGGER task_validation_command_results_sequence_insert
BEFORE INSERT ON task_validation_command_results
WHEN NEW.attempt_sequence != (
    SELECT COUNT(*) + 1 FROM task_validation_command_results
    WHERE task_id = NEW.task_id
      AND approved_task_version = NEW.approved_task_version
      AND execution_scope = NEW.execution_scope
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

CREATE TABLE task_post_merge_validation_results (
    task_id TEXT NOT NULL,
    approval_task_version INTEGER NOT NULL CHECK (approval_task_version >= 0),
    post_merge_task_version INTEGER NOT NULL CHECK (post_merge_task_version >= 0),
    execution_scope TEXT NOT NULL CHECK (execution_scope = 'ProjectRoot'),
    command_kind TEXT NOT NULL CHECK (
        command_kind IN ('Format', 'Lint', 'Typecheck', 'Test', 'Build')
    ),
    attempt_sequence INTEGER NOT NULL CHECK (attempt_sequence >= 1),
    outcome TEXT NOT NULL CHECK (
        outcome IN (
            'Success', 'ExitFailure', 'TimedOut', 'StdoutBoundExceeded',
            'BindingRejected', 'Cancelled', 'Uncertain'
        )
    ),
    exit_code INTEGER NULL,
    safe_summary TEXT NOT NULL CHECK (length(safe_summary) BETWEEN 1 AND 2000),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    completed_at_ms INTEGER NOT NULL CHECK (completed_at_ms >= started_at_ms),
    PRIMARY KEY (
        task_id, approval_task_version, post_merge_task_version,
        command_kind, attempt_sequence
    ),
    CHECK (
        (outcome IN ('Success', 'ExitFailure') AND exit_code IS NOT NULL)
        OR (outcome NOT IN ('Success', 'ExitFailure') AND exit_code IS NULL)
    ),
    FOREIGN KEY (task_id, approval_task_version, execution_scope, command_kind)
        REFERENCES task_validation_command_approvals (
            task_id, approved_task_version, execution_scope, command_kind
        )
);

CREATE TRIGGER task_post_merge_validation_results_sequence_insert
BEFORE INSERT ON task_post_merge_validation_results
WHEN NEW.attempt_sequence != (
    SELECT COUNT(*) + 1 FROM task_post_merge_validation_results
    WHERE task_id = NEW.task_id
      AND approval_task_version = NEW.approval_task_version
      AND post_merge_task_version = NEW.post_merge_task_version
      AND command_kind = NEW.command_kind
)
BEGIN
    SELECT RAISE(ABORT, 'post-merge validation result attempt sequence mismatch');
END;

CREATE TRIGGER task_post_merge_validation_results_immutable_update
BEFORE UPDATE ON task_post_merge_validation_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_post_merge_validation_results is immutable');
END;

CREATE TRIGGER task_post_merge_validation_results_immutable_delete
BEFORE DELETE ON task_post_merge_validation_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_post_merge_validation_results is immutable');
END;

CREATE INDEX task_post_merge_validation_results_task_id_idx
    ON task_post_merge_validation_results (task_id);
