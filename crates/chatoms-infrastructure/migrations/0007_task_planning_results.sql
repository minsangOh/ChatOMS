CREATE TABLE task_planning_results (
    task_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider = 'Claude'),
    work_kind TEXT NOT NULL CHECK (work_kind = 'Planning'),
    outcome TEXT NOT NULL CHECK (
        outcome IN ('Completed', 'Failed', 'Cancelled', 'RecoveryRequired')
    ),
    exit_code INTEGER NULL,
    turn_count INTEGER NULL CHECK (turn_count IS NULL OR turn_count >= 0),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    completed_at_ms INTEGER NOT NULL CHECK (completed_at_ms >= started_at_ms),
    plan_text TEXT NULL CHECK (
        (outcome = 'Completed'
            AND plan_text IS NOT NULL
            AND length(plan_text) BETWEEN 1 AND 100000)
        OR (outcome != 'Completed' AND plan_text IS NULL)
    ),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_planning_results_immutable_update
BEFORE UPDATE ON task_planning_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_planning_results is immutable');
END;

CREATE TRIGGER task_planning_results_immutable_delete
BEFORE DELETE ON task_planning_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_planning_results is immutable');
END;
