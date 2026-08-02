mod support;

use std::{
    str::FromStr,
    sync::{Arc, Barrier, mpsc},
    thread,
    time::Duration,
};

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskId, TaskState, TaskStateTransition,
    TaskStateTransitionId, TaskStateTransitionSnapshot,
};
use chatoms_infrastructure::database::{DatabaseConnection, SqliteFoundationRepository};
use chatoms_ports::repository::{FoundationRepository, RepositoryErrorCode};

use support::{TestDatabase, count_rows, foreign_key_violation_count, insert_project};

fn initial(task: &Task) -> TaskStateTransition {
    TaskStateTransition::initial(
        TaskStateTransitionId::new(),
        task.id(),
        ActorKind::from_str("application").expect("actor"),
        ReasonCode::from_str("task.created").expect("reason"),
        task.created_at_ms(),
    )
}

#[test]
fn two_connections_creating_active_tasks_allow_exactly_one_commit() {
    let database = TestDatabase::migrated();
    let project_id = ProjectId::new();
    insert_project(&database.open_raw(), &project_id.to_string());
    let path = Arc::new(database.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();

    for _ in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        handles.push(thread::spawn(move || {
            let task = Task::new(TaskId::new(), project_id, 100);
            let transition = initial(&task);
            let mut database = DatabaseConnection::open(path.as_ref()).expect("open connection");
            let mut repository = SqliteFoundationRepository::new(&mut database);
            barrier.wait();
            let result = repository
                .create_task(&task, &transition, 100)
                .map(|()| task.id())
                .map_err(|error| error.code());
            sender.send(result).expect("send create result");
        }));
    }
    drop(sender);

    let results = (0..2)
        .map(|_| {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrent create timed out")
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("create thread panicked");
    }

    let successes = results.iter().filter(|result| result.is_ok()).count();
    let lease_conflicts = results
        .iter()
        .filter(|result| matches!(result, Err(RepositoryErrorCode::ActiveLeaseConflict)))
        .count();
    println!("concurrent create: successes={successes}, lease_conflicts={lease_conflicts}");
    assert_eq!(successes, 1);
    assert_eq!(lease_conflicts, 1);

    let connection = database.open_raw();
    assert_eq!(count_rows(&connection, "tasks"), 1);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 1);
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
    assert_eq!(foreign_key_violation_count(&connection), 0);
}

#[test]
fn two_connections_transitioning_same_version_allow_exactly_one_commit() {
    let database = TestDatabase::migrated();
    let project_id = ProjectId::new();
    insert_project(&database.open_raw(), &project_id.to_string());
    let original = Task::new(TaskId::new(), project_id, 100);
    let initial = initial(&original);
    {
        let mut connection = DatabaseConnection::open(database.path()).expect("open setup DB");
        let mut repository = SqliteFoundationRepository::new(&mut connection);
        repository
            .create_task(&original, &initial, 100)
            .expect("create original task");
    }

    let path = Arc::new(database.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        let mut task = original.clone();
        handles.push(thread::spawn(move || {
            task.transition_to(TaskState::ProjectValidated, 110)
                .expect("domain transition");
            let transition = TaskStateTransition::new(TaskStateTransitionSnapshot {
                id: TaskStateTransitionId::new(),
                task_id: task.id(),
                sequence: 2,
                from_state: Some(TaskState::Created),
                to_state: TaskState::ProjectValidated,
                task_version: 1,
                actor_kind: ActorKind::from_str("application").expect("actor"),
                reason_code: ReasonCode::from_str("task.transition").expect("reason"),
                occurred_at_ms: 110,
            })
            .expect("transition record");
            let mut database = DatabaseConnection::open(path.as_ref()).expect("open connection");
            let mut repository = SqliteFoundationRepository::new(&mut database);
            barrier.wait();
            let result = repository
                .save_transition(0, &task, &transition)
                .map_err(|error| error.code());
            sender.send(result).expect("send transition result");
        }));
    }
    drop(sender);

    let results = (0..2)
        .map(|_| {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("concurrent transition timed out")
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("transition thread panicked");
    }

    let successes = results.iter().filter(|result| result.is_ok()).count();
    let version_conflicts = results
        .iter()
        .filter(|result| matches!(result, Err(RepositoryErrorCode::VersionConflict)))
        .count();
    println!("concurrent transition: successes={successes}, version_conflicts={version_conflicts}");
    assert_eq!(successes, 1);
    assert_eq!(version_conflicts, 1);

    let connection = database.open_raw();
    assert_eq!(count_rows(&connection, "tasks"), 1);
    assert_eq!(count_rows(&connection, "task_state_transitions"), 2);
    assert_eq!(count_rows(&connection, "active_task_leases"), 1);
    let (version, maximum_sequence): (i64, i64) = connection
        .query_row(
            "SELECT tasks.version, MAX(task_state_transitions.sequence)
             FROM tasks
             JOIN task_state_transitions ON task_state_transitions.task_id = tasks.id
             WHERE tasks.id = ?1",
            [original.id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read final version and sequence");
    assert_eq!((version, maximum_sequence), (1, 2));
    assert_eq!(foreign_key_violation_count(&connection), 0);
}
