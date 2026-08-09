ALTER TABLE projects ADD COLUMN canonical_path_key TEXT;
ALTER TABLE projects ADD COLUMN display_path TEXT;

CREATE UNIQUE INDEX projects_canonical_path_key_unique
ON projects (canonical_path_key);

CREATE TRIGGER projects_phase2_fields_required_insert
BEFORE INSERT ON projects
WHEN NEW.canonical_path_key IS NULL
    OR length(NEW.canonical_path_key) = 0
    OR NEW.display_path IS NULL
    OR length(NEW.display_path) = 0
BEGIN
    SELECT RAISE(ABORT, 'projects Phase 2 path fields are required');
END;

CREATE TRIGGER projects_phase2_fields_required_update
BEFORE UPDATE OF canonical_path_key, display_path ON projects
WHEN NEW.canonical_path_key IS NULL
    OR length(NEW.canonical_path_key) = 0
    OR NEW.display_path IS NULL
    OR length(NEW.display_path) = 0
BEGIN
    SELECT RAISE(ABORT, 'projects Phase 2 path fields are required');
END;

CREATE TRIGGER projects_root_identity_immutable
BEFORE UPDATE OF root_path, canonical_path_key ON projects
WHEN NEW.root_path IS NOT OLD.root_path
    OR (OLD.canonical_path_key IS NOT NULL
        AND NEW.canonical_path_key IS NOT OLD.canonical_path_key)
BEGIN
    SELECT RAISE(ABORT, 'project root identity is immutable');
END;

CREATE TABLE project_filesystem_identities (
    project_id TEXT PRIMARY KEY,
    identity_scheme TEXT NOT NULL CHECK (identity_scheme = 'WindowsFileIdV1'),
    root_volume_serial_hex TEXT NOT NULL CHECK (
        length(root_volume_serial_hex) = 16
        AND root_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
    ),
    root_file_id_hex TEXT NOT NULL CHECK (
        length(root_file_id_hex) = 32
        AND root_file_id_hex NOT GLOB '*[^0-9a-f]*'
    ),
    repository_kind TEXT NOT NULL CHECK (repository_kind IN ('Git', 'NonGit')),
    git_common_volume_serial_hex TEXT NULL CHECK (
        git_common_volume_serial_hex IS NULL OR (
            length(git_common_volume_serial_hex) = 16
            AND git_common_volume_serial_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    git_common_file_id_hex TEXT NULL CHECK (
        git_common_file_id_hex IS NULL OR (
            length(git_common_file_id_hex) = 32
            AND git_common_file_id_hex NOT GLOB '*[^0-9a-f]*'
        )
    ),
    confirmed INTEGER NOT NULL CHECK (confirmed IN (0, 1)),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    verified_at_ms INTEGER NOT NULL CHECK (verified_at_ms >= 0),
    CHECK (
        (repository_kind = 'Git'
            AND git_common_volume_serial_hex IS NOT NULL
            AND git_common_file_id_hex IS NOT NULL)
        OR (repository_kind = 'NonGit'
            AND git_common_volume_serial_hex IS NULL
            AND git_common_file_id_hex IS NULL)
    ),
    UNIQUE (identity_scheme, root_volume_serial_hex, root_file_id_hex),
    FOREIGN KEY (project_id) REFERENCES projects (id)
);

CREATE TRIGGER project_filesystem_identity_key_immutable
BEFORE UPDATE OF project_id, identity_scheme, root_volume_serial_hex, root_file_id_hex
ON project_filesystem_identities
WHEN NEW.project_id IS NOT OLD.project_id
    OR NEW.identity_scheme IS NOT OLD.identity_scheme
    OR NEW.root_volume_serial_hex IS NOT OLD.root_volume_serial_hex
    OR NEW.root_file_id_hex IS NOT OLD.root_file_id_hex
BEGIN
    SELECT RAISE(ABORT, 'project filesystem root identity is immutable');
END;

CREATE TRIGGER project_filesystem_identity_insert_confirmed
BEFORE INSERT ON project_filesystem_identities
WHEN NEW.confirmed != 1
BEGIN
    SELECT RAISE(ABORT, 'project filesystem identity must be confirmed');
END;

CREATE TRIGGER project_filesystem_identity_update_policy
BEFORE UPDATE ON project_filesystem_identities
WHEN NEW.confirmed != 1
    OR NEW.revision != OLD.revision + 1
    OR (OLD.repository_kind = 'Git' AND NEW.repository_kind != 'Git')
BEGIN
    SELECT RAISE(ABORT, 'project filesystem identity update violates policy');
END;

CREATE UNIQUE INDEX tasks_id_project_id_unique
ON tasks (id, project_id);

CREATE TRIGGER tasks_confirmed_project_identity_insert
BEFORE INSERT ON tasks
WHEN NOT EXISTS (
    SELECT 1 FROM project_filesystem_identities AS identity
    WHERE identity.project_id = NEW.project_id AND identity.confirmed = 1
)
BEGIN
    SELECT RAISE(ABORT, 'task project filesystem identity is not confirmed');
END;

CREATE TABLE task_git_isolations (
    task_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'AwaitingGitInitApproval',
        'Ready',
        'GitInitInProgress',
        'WorktreeCreating',
        'WorktreeReady',
        'RecoveryRequired'
    )),
    operation_id TEXT NULL,
    expected_task_version INTEGER NOT NULL CHECK (expected_task_version >= 0),
    base_branch TEXT NULL CHECK (base_branch IS NULL OR length(base_branch) > 0),
    base_commit TEXT NULL CHECK (
        base_commit IS NULL OR (
            length(base_commit) IN (40, 64)
            AND base_commit NOT GLOB '*[^0-9a-f]*'
        )
    ),
    worktree_path TEXT NULL UNIQUE CHECK (
        worktree_path IS NULL OR length(worktree_path) > 0
    ),
    branch_created_by_app INTEGER NOT NULL DEFAULT 0 CHECK (
        branch_created_by_app IN (0, 1)
    ),
    worktree_created_by_app INTEGER NOT NULL DEFAULT 0 CHECK (
        worktree_created_by_app IN (0, 1)
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (
        updated_at_ms >= 0 AND updated_at_ms >= created_at_ms
    ),
    CHECK (
        (status = 'AwaitingGitInitApproval'
            AND operation_id IS NULL
            AND base_branch IS NULL
            AND base_commit IS NULL
            AND worktree_path IS NULL
            AND branch_created_by_app = 0
            AND worktree_created_by_app = 0)
        OR (status = 'Ready'
            AND base_branch IS NULL
            AND base_commit IS NULL
            AND worktree_path IS NULL
            AND branch_created_by_app = 0
            AND worktree_created_by_app = 0)
        OR (status = 'GitInitInProgress'
            AND operation_id IS NOT NULL
            AND base_branch IS NULL
            AND base_commit IS NULL
            AND worktree_path IS NULL
            AND branch_created_by_app = 0
            AND worktree_created_by_app = 0)
        OR (status = 'WorktreeCreating'
            AND operation_id IS NOT NULL
            AND base_branch IS NOT NULL
            AND base_commit IS NOT NULL
            AND worktree_path IS NOT NULL
            AND branch_created_by_app = 0
            AND worktree_created_by_app = 0)
        OR (status = 'WorktreeReady'
            AND operation_id IS NOT NULL
            AND base_branch IS NOT NULL
            AND base_commit IS NOT NULL
            AND worktree_path IS NOT NULL
            AND branch_created_by_app = 1
            AND worktree_created_by_app = 1)
        OR (status = 'RecoveryRequired'
            AND operation_id IS NOT NULL
            AND branch_created_by_app = 0
            AND worktree_created_by_app = 0
            AND ((base_branch IS NULL AND base_commit IS NULL AND worktree_path IS NULL)
                OR (base_branch IS NOT NULL AND base_commit IS NOT NULL AND worktree_path IS NOT NULL)))
    ),
    FOREIGN KEY (task_id, project_id) REFERENCES tasks (id, project_id)
);

CREATE TRIGGER task_git_isolation_identity_immutable
BEFORE UPDATE OF task_id, project_id, worktree_path ON task_git_isolations
WHEN NEW.task_id IS NOT OLD.task_id
    OR NEW.project_id IS NOT OLD.project_id
    OR (OLD.worktree_path IS NOT NULL AND NEW.worktree_path IS NOT OLD.worktree_path)
BEGIN
    SELECT RAISE(ABORT, 'task Git isolation identity is immutable');
END;

CREATE TRIGGER task_git_isolation_base_immutable
BEFORE UPDATE OF base_branch, base_commit ON task_git_isolations
WHEN (OLD.base_branch IS NOT NULL AND NEW.base_branch IS NOT OLD.base_branch)
    OR (OLD.base_commit IS NOT NULL AND NEW.base_commit IS NOT OLD.base_commit)
BEGIN
    SELECT RAISE(ABORT, 'task Git isolation base is immutable');
END;

CREATE TABLE git_init_approvals (
    operation_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    approved_at_ms INTEGER NOT NULL CHECK (approved_at_ms >= 0),
    FOREIGN KEY (task_id, project_id) REFERENCES tasks (id, project_id)
);

CREATE TABLE git_operation_attempts (
    operation_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('GitInitialize', 'WorktreeCreate')),
    status TEXT NOT NULL CHECK (status IN ('IntentRecorded', 'RecoveryRequired', 'Completed')),
    approved_task_version INTEGER NOT NULL CHECK (approved_task_version >= 0),
    project_identity_revision INTEGER NOT NULL CHECK (project_identity_revision >= 1),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE (task_id, operation_kind),
    FOREIGN KEY (task_id, project_id) REFERENCES tasks (id, project_id)
);

CREATE TABLE git_operation_receipts (
    operation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    receipt_kind TEXT NOT NULL CHECK (receipt_kind IN (
        'CommandStarted', 'CommandSucceeded', 'PostVerified',
        'CompletionRecorded', 'RecoveryRequired'
    )),
    evidence TEXT NULL CHECK (evidence IS NULL OR length(evidence) > 0),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    PRIMARY KEY (operation_id, sequence),
    FOREIGN KEY (operation_id) REFERENCES git_operation_attempts (operation_id)
);

CREATE TRIGGER git_operation_attempt_identity_immutable
BEFORE UPDATE OF operation_id, task_id, project_id, operation_kind,
    approved_task_version, project_identity_revision, created_at_ms
ON git_operation_attempts
BEGIN
    SELECT RAISE(ABORT, 'Git operation attempt identity is immutable');
END;

CREATE TRIGGER git_operation_attempt_binding_insert
BEFORE INSERT ON git_operation_attempts
WHEN NOT EXISTS (
    SELECT 1
    FROM task_git_isolations AS isolation
    JOIN tasks AS task ON task.id = isolation.task_id
    JOIN project_filesystem_identities AS identity ON identity.project_id = isolation.project_id
    WHERE isolation.task_id = NEW.task_id
      AND isolation.project_id = NEW.project_id
      AND isolation.operation_id = NEW.operation_id
      AND isolation.expected_task_version = NEW.approved_task_version
      AND task.version = NEW.approved_task_version
      AND identity.revision = NEW.project_identity_revision
      AND identity.confirmed = 1
      AND ((NEW.operation_kind = 'GitInitialize' AND isolation.status = 'GitInitInProgress')
        OR (NEW.operation_kind = 'WorktreeCreate' AND isolation.status = 'WorktreeCreating'))
)
BEGIN
    SELECT RAISE(ABORT, 'Git operation attempt binding mismatch');
END;

CREATE TRIGGER git_operation_receipt_sequence_insert
BEFORE INSERT ON git_operation_receipts
WHEN NEW.sequence != (
    SELECT COUNT(*) + 1 FROM git_operation_receipts
    WHERE operation_id = NEW.operation_id
)
BEGIN
    SELECT RAISE(ABORT, 'Git operation receipt sequence mismatch');
END;

CREATE TRIGGER git_operation_receipt_evidence_insert
BEFORE INSERT ON git_operation_receipts
WHEN (NEW.receipt_kind = 'CommandSucceeded'
        AND EXISTS (
            SELECT 1 FROM git_operation_attempts
            WHERE operation_id = NEW.operation_id AND operation_kind = 'GitInitialize'
        )
        AND (NEW.evidence IS NULL
            OR length(NEW.evidence) NOT IN (40, 64)
            OR NEW.evidence GLOB '*[^0-9a-f]*'))
    OR (NEW.receipt_kind != 'CommandSucceeded' AND NEW.evidence IS NOT NULL)
    OR (NEW.receipt_kind = 'CommandSucceeded'
        AND EXISTS (
            SELECT 1 FROM git_operation_attempts
            WHERE operation_id = NEW.operation_id AND operation_kind = 'WorktreeCreate'
        )
        AND NEW.evidence IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'Git operation receipt evidence mismatch');
END;

CREATE TRIGGER git_operation_receipt_immutable
BEFORE UPDATE ON git_operation_receipts
BEGIN
    SELECT RAISE(ABORT, 'Git operation receipt is immutable');
END;

CREATE TRIGGER git_operation_receipt_no_delete
BEFORE DELETE ON git_operation_receipts
BEGIN
    SELECT RAISE(ABORT, 'Git operation receipt cannot be deleted');
END;

CREATE TRIGGER git_init_approval_binding_insert
BEFORE INSERT ON git_init_approvals
WHEN NOT EXISTS (
    SELECT 1
    FROM tasks AS task
    JOIN task_git_isolations AS isolation ON isolation.task_id = task.id
    WHERE task.id = NEW.task_id
      AND task.project_id = NEW.project_id
      AND task.version = NEW.approved_task_version
      AND isolation.project_id = NEW.project_id
      AND isolation.operation_id = NEW.operation_id
      AND isolation.expected_task_version = NEW.approved_task_version
      AND isolation.status = 'GitInitInProgress'
)
BEGIN
    SELECT RAISE(ABORT, 'Git initialization approval binding mismatch');
END;

CREATE TRIGGER git_init_approval_immutable
BEFORE UPDATE ON git_init_approvals
BEGIN
    SELECT RAISE(ABORT, 'Git initialization approval is immutable');
END;
