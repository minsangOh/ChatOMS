CREATE TABLE tasks_v4 (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'Created',
        'ProjectValidated',
        'AwaitingGitInitApproval',
        'GitInitialized',
        'WorktreeCreating',
        'WorktreeReady',
        'Planning',
        'AwaitingDesignApproval',
        'Implementing',
        'Testing',
        'AutoFixing',
        'Reviewing',
        'ReviewFixing',
        'AwaitingUserDiffApproval',
        'Merging',
        'MergeConflict',
        'PostMergeTesting',
        'Completed',
        'Paused',
        'Failed',
        'RecoveryRequired',
        'UnknownExternalEffect',
        'Cancelled',
        'CleanupPending',
        'Archived'
    )),
    version INTEGER NOT NULL CHECK (version >= 0),
    task_branch_identity TEXT NOT NULL UNIQUE CHECK (
        length(task_branch_identity) > length('ai-task/')
        AND substr(task_branch_identity, 1, length('ai-task/')) = 'ai-task/'
    ),
    resume_target_state TEXT NULL CHECK (
        resume_target_state IS NULL OR resume_target_state IN (
            'Created',
            'ProjectValidated',
            'AwaitingGitInitApproval',
            'GitInitialized',
            'WorktreeCreating',
            'WorktreeReady',
            'Planning',
            'AwaitingDesignApproval',
            'Implementing',
            'Testing',
            'AutoFixing',
            'Reviewing',
            'ReviewFixing',
            'AwaitingUserDiffApproval',
            'Merging',
            'MergeConflict',
            'PostMergeTesting',
            'Completed',
            'Paused',
            'Failed',
            'RecoveryRequired',
            'UnknownExternalEffect',
            'Cancelled',
            'CleanupPending',
            'Archived'
        )
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (
        updated_at_ms >= 0 AND updated_at_ms >= created_at_ms
    ),
    terminal_at_ms INTEGER NULL CHECK (
        terminal_at_ms IS NULL OR terminal_at_ms >= created_at_ms
    ),
    lease_required_key INTEGER GENERATED ALWAYS AS (
        CASE
            WHEN state IN ('Completed', 'Failed', 'Cancelled', 'CleanupPending', 'Archived')
                THEN NULL
            ELSE 1
        END
    ) STORED,
    CHECK (
        (state = 'Paused' AND resume_target_state IS NOT NULL)
        OR state = 'RecoveryRequired'
        OR (state NOT IN ('Paused', 'RecoveryRequired') AND resume_target_state IS NULL)
    ),
    CHECK (
        (
            state IN ('Completed', 'Failed', 'Cancelled', 'CleanupPending', 'Archived')
            AND terminal_at_ms IS NOT NULL
        )
        OR (
            state NOT IN ('Completed', 'Failed', 'Cancelled', 'CleanupPending', 'Archived')
            AND terminal_at_ms IS NULL
        )
    ),
    UNIQUE (id, lease_required_key),
    FOREIGN KEY (project_id) REFERENCES projects (id),
    FOREIGN KEY (id, lease_required_key)
        REFERENCES active_task_leases (task_id, singleton_key)
        DEFERRABLE INITIALLY DEFERRED
);

INSERT INTO tasks_v4 (
    id,
    project_id,
    state,
    version,
    task_branch_identity,
    resume_target_state,
    created_at_ms,
    updated_at_ms,
    terminal_at_ms
)
SELECT
    id,
    project_id,
    CASE state
        WHEN 'PlanningWithClaude' THEN 'Planning'
        WHEN 'ImplementingWithCodex' THEN 'Implementing'
        WHEN 'ReviewingWithClaude' THEN 'Reviewing'
        ELSE state
    END,
    version,
    task_branch_identity,
    CASE resume_target_state
        WHEN 'PlanningWithClaude' THEN 'Planning'
        WHEN 'ImplementingWithCodex' THEN 'Implementing'
        WHEN 'ReviewingWithClaude' THEN 'Reviewing'
        ELSE resume_target_state
    END,
    created_at_ms,
    updated_at_ms,
    terminal_at_ms
FROM tasks;


CREATE TABLE task_state_transitions_v4 (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    from_state TEXT NULL CHECK (
        from_state IS NULL OR from_state IN (
            'Created',
            'ProjectValidated',
            'AwaitingGitInitApproval',
            'GitInitialized',
            'WorktreeCreating',
            'WorktreeReady',
            'Planning',
            'AwaitingDesignApproval',
            'Implementing',
            'Testing',
            'AutoFixing',
            'Reviewing',
            'ReviewFixing',
            'AwaitingUserDiffApproval',
            'Merging',
            'MergeConflict',
            'PostMergeTesting',
            'Completed',
            'Paused',
            'Failed',
            'RecoveryRequired',
            'UnknownExternalEffect',
            'Cancelled',
            'CleanupPending',
            'Archived'
        )
    ),
    to_state TEXT NOT NULL CHECK (to_state IN (
        'Created',
        'ProjectValidated',
        'AwaitingGitInitApproval',
        'GitInitialized',
        'WorktreeCreating',
        'WorktreeReady',
        'Planning',
        'AwaitingDesignApproval',
        'Implementing',
        'Testing',
        'AutoFixing',
        'Reviewing',
        'ReviewFixing',
        'AwaitingUserDiffApproval',
        'Merging',
        'MergeConflict',
        'PostMergeTesting',
        'Completed',
        'Paused',
        'Failed',
        'RecoveryRequired',
        'UnknownExternalEffect',
        'Cancelled',
        'CleanupPending',
        'Archived'
    )),
    task_version INTEGER NOT NULL CHECK (task_version >= 0),
    actor_kind TEXT NOT NULL CHECK (length(actor_kind) BETWEEN 1 AND 64),
    reason_code TEXT NOT NULL CHECK (length(reason_code) BETWEEN 1 AND 128),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    CHECK (
        (
            sequence = 1
            AND from_state IS NULL
            AND to_state = 'Created'
            AND task_version = 0
        )
        OR (
            sequence > 1
            AND from_state IS NOT NULL
            AND task_version >= 1
        )
    ),
    UNIQUE (task_id, sequence),
    UNIQUE (task_id, task_version),
    FOREIGN KEY (task_id) REFERENCES tasks (id) DEFERRABLE INITIALLY DEFERRED
);

INSERT INTO task_state_transitions_v4 (
    id,
    task_id,
    sequence,
    from_state,
    to_state,
    task_version,
    actor_kind,
    reason_code,
    occurred_at_ms
)
SELECT
    id,
    task_id,
    sequence,
    CASE from_state
        WHEN 'PlanningWithClaude' THEN 'Planning'
        WHEN 'ImplementingWithCodex' THEN 'Implementing'
        WHEN 'ReviewingWithClaude' THEN 'Reviewing'
        ELSE from_state
    END,
    CASE to_state
        WHEN 'PlanningWithClaude' THEN 'Planning'
        WHEN 'ImplementingWithCodex' THEN 'Implementing'
        WHEN 'ReviewingWithClaude' THEN 'Reviewing'
        ELSE to_state
    END,
    task_version,
    actor_kind,
    reason_code,
    occurred_at_ms
FROM task_state_transitions;


DROP TRIGGER active_lease_nonterminal_delete_guard;
DROP TRIGGER active_lease_terminal_insert_guard;
DROP TRIGGER git_operation_attempt_binding_insert;
DROP TRIGGER git_init_approval_binding_insert;
DROP TABLE task_state_transitions;
DROP TABLE tasks;

ALTER TABLE tasks_v4 RENAME TO tasks;
ALTER TABLE task_state_transitions_v4 RENAME TO task_state_transitions;

CREATE TRIGGER tasks_project_id_immutable
BEFORE UPDATE OF project_id ON tasks
FOR EACH ROW
WHEN NEW.project_id IS NOT OLD.project_id
BEGIN
    SELECT RAISE(ABORT, 'tasks.project_id is immutable');
END;

CREATE TRIGGER tasks_branch_identity_immutable
BEFORE UPDATE OF task_branch_identity ON tasks
FOR EACH ROW
WHEN NEW.task_branch_identity IS NOT OLD.task_branch_identity
BEGIN
    SELECT RAISE(ABORT, 'tasks.task_branch_identity is immutable');
END;

CREATE TRIGGER tasks_created_at_immutable
BEFORE UPDATE OF created_at_ms ON tasks
FOR EACH ROW
WHEN NEW.created_at_ms IS NOT OLD.created_at_ms
BEGIN
    SELECT RAISE(ABORT, 'tasks.created_at_ms is immutable');
END;

CREATE TRIGGER tasks_confirmed_project_identity_insert
BEFORE INSERT ON tasks
WHEN NOT EXISTS (
    SELECT 1 FROM project_filesystem_identities AS identity
    WHERE identity.project_id = NEW.project_id AND identity.confirmed = 1
)
BEGIN
    SELECT RAISE(ABORT, 'task project filesystem identity is not confirmed');
END;

CREATE TRIGGER active_lease_nonterminal_delete_guard
BEFORE DELETE ON active_task_leases
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM tasks
    WHERE id = OLD.task_id AND lease_required_key = 1
)
BEGIN
    SELECT RAISE(ABORT, 'nonterminal task lease cannot be deleted');
END;

CREATE TRIGGER active_lease_terminal_insert_guard
BEFORE INSERT ON active_task_leases
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM tasks
    WHERE id = NEW.task_id AND lease_required_key IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'terminal task cannot acquire an active lease');
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

CREATE INDEX tasks_project_id_idx ON tasks (project_id);
CREATE INDEX tasks_state_idx ON tasks (state);
CREATE UNIQUE INDEX tasks_id_project_id_unique ON tasks (id, project_id);
