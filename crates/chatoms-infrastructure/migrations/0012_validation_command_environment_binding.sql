-- Widens task_validation_command_approvals (given executable/tool-directory
-- binding in 0011) to also bind the approved CARGO_HOME and RUSTUP_HOME
-- environment directories a Cargo validation command run may rely on: each
-- home's approved absolute path and Windows stable NTFS object identity
-- (volume serial + file ID). Unit 4d-2a's CargoValidationAdapter re-verified
-- an environment-directory binding immediately before every spawn, but that
-- binding was only ever supplied by the adapter's caller at construction
-- time and was never tied to this durable, user-approved,
-- per-(task, version, command_kind) row -- so the re-verification compared
-- a fresh identity against itself, not against anything the user had
-- actually approved. This migration closes that gap by making both homes
-- part of the same immutable approval row.
--
-- Both homes are optional per approval (a validation command may run with
-- no explicit CARGO_HOME/RUSTUP_HOME override), so each home's three
-- columns are NULL-able but constrained to be all-NULL or all-NOT-NULL
-- together, mirroring 0002_git_isolation.sql's
-- git_common_volume_serial_hex/git_common_file_id_hex optional-pair
-- convention.
--
-- Unlike 0011 (which dropped any pre-existing row as dev/test scratch data,
-- since no shipped UI had ever written one), this table may already hold
-- real user-approved rows by the time this migration runs. This migration
-- must never fabricate environment identity for an existing row, so it
-- aborts before touching the table if a pre-existing row is found. SQLite's
-- RAISE() is only usable inside a trigger body, so this precondition is
-- enforced in `chatoms_infrastructure::database::migration` (in Rust)
-- rather than in this SQL file.
CREATE TABLE task_validation_command_approvals_v12 (
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
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    PRIMARY KEY (task_id, approved_task_version, command_kind),
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
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

DROP TRIGGER task_validation_command_approvals_binding_insert;
DROP TRIGGER task_validation_command_approvals_immutable_update;
DROP TRIGGER task_validation_command_approvals_immutable_delete;
DROP INDEX task_validation_command_approvals_task_id_idx;
DROP TABLE task_validation_command_approvals;

ALTER TABLE task_validation_command_approvals_v12
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
