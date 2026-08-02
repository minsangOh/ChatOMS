CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    root_path TEXT NOT NULL CHECK (length(root_path) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (
        updated_at_ms >= 0 AND updated_at_ms >= created_at_ms
    )
);

CREATE TABLE app_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (
        updated_at_ms >= 0 AND updated_at_ms >= created_at_ms
    )
);

CREATE TABLE provider_bindings (
    id TEXT PRIMARY KEY,
    app_profile_id TEXT NOT NULL,
    provider_kind TEXT NOT NULL CHECK (length(provider_kind) > 0),
    display_name TEXT NOT NULL CHECK (length(display_name) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (
        updated_at_ms >= 0 AND updated_at_ms >= created_at_ms
    ),
    FOREIGN KEY (app_profile_id) REFERENCES app_profiles (id)
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'Created',
        'ProjectValidated',
        'AwaitingGitInitApproval',
        'GitInitialized',
        'WorktreeCreating',
        'WorktreeReady',
        'PlanningWithClaude',
        'AwaitingDesignApproval',
        'ImplementingWithCodex',
        'Testing',
        'AutoFixing',
        'ReviewingWithClaude',
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
            'PlanningWithClaude',
            'AwaitingDesignApproval',
            'ImplementingWithCodex',
            'Testing',
            'AutoFixing',
            'ReviewingWithClaude',
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

CREATE TABLE active_task_leases (
    singleton_key INTEGER PRIMARY KEY CHECK (singleton_key = 1),
    task_id TEXT NOT NULL,
    acquired_at_ms INTEGER NOT NULL CHECK (acquired_at_ms >= 0),
    UNIQUE (task_id, singleton_key),
    FOREIGN KEY (task_id, singleton_key)
        REFERENCES tasks (id, lease_required_key)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE task_state_transitions (
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
            'PlanningWithClaude',
            'AwaitingDesignApproval',
            'ImplementingWithCodex',
            'Testing',
            'AutoFixing',
            'ReviewingWithClaude',
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
        'PlanningWithClaude',
        'AwaitingDesignApproval',
        'ImplementingWithCodex',
        'Testing',
        'AutoFixing',
        'ReviewingWithClaude',
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

CREATE INDEX tasks_project_id_idx ON tasks (project_id);
CREATE INDEX tasks_state_idx ON tasks (state);
CREATE INDEX provider_bindings_app_profile_id_idx ON provider_bindings (app_profile_id);
