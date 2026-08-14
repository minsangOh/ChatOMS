-- Immutable, 1:1-per-task record of a Claude Review attempt's safe final
-- result. review_text is masked and size-bounded by the application layer
-- before it ever reaches this table (mirroring task_planning_results.plan_text)
-- and is present only when outcome = 'Completed'. Raw stdout/stderr, raw Git
-- diff, transcript, tool I/O, prompt text, executable/environment path, and
-- login/session/cost information must never be stored in this table.
CREATE TABLE task_review_results (
    task_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider = 'Claude'),
    work_kind TEXT NOT NULL CHECK (work_kind = 'Review'),
    outcome TEXT NOT NULL CHECK (
        outcome IN ('Completed', 'Failed', 'Cancelled', 'RecoveryRequired')
    ),
    exit_code INTEGER NULL,
    turn_count INTEGER NULL CHECK (turn_count IS NULL OR turn_count >= 0),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    completed_at_ms INTEGER NOT NULL CHECK (completed_at_ms >= started_at_ms),
    -- Masked, size-bounded final review text. Present only for a Completed
    -- outcome; every other outcome must leave this NULL.
    review_text TEXT NULL CHECK (
        (outcome = 'Completed'
            AND review_text IS NOT NULL
            AND length(review_text) BETWEEN 1 AND 100000)
        OR (outcome != 'Completed' AND review_text IS NULL)
    ),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_review_results_immutable_update
BEFORE UPDATE ON task_review_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_review_results is immutable');
END;

CREATE TRIGGER task_review_results_immutable_delete
BEFORE DELETE ON task_review_results
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_review_results is immutable');
END;
