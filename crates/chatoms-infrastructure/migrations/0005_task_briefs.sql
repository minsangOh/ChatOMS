CREATE TABLE task_briefs (
    task_id TEXT PRIMARY KEY,
    requirements TEXT NOT NULL CHECK (length(requirements) > 0),
    completion_criteria TEXT NOT NULL CHECK (length(completion_criteria) > 0),
    prohibited_scope TEXT NOT NULL CHECK (length(prohibited_scope) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    FOREIGN KEY (task_id) REFERENCES tasks (id)
);

CREATE TRIGGER task_briefs_immutable_update
BEFORE UPDATE ON task_briefs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_briefs is immutable');
END;

CREATE TRIGGER task_briefs_immutable_delete
BEFORE DELETE ON task_briefs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'task_briefs is immutable');
END;
