#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId, TaskSnapshot, TaskState,
    TaskStateTransition, TaskStateTransitionId,
};
use chatoms_ports::{
    TimeProvider,
    error::{FailureCategory, PortFailure},
    repository::{
        ActiveLease, FoundationRepository, ProjectSummary, RepositoryError, RepositoryErrorCode,
    },
};

#[derive(Default)]
pub struct FakeRepository {
    pub projects: Vec<ProjectSummary>,
    pub tasks: HashMap<TaskId, Task>,
    pub transitions: HashMap<TaskId, Vec<TaskStateTransition>>,
    pub active_lease: Option<ActiveLease>,
    pub calls: Vec<&'static str>,
    pub shared_calls: Option<Arc<Mutex<Vec<&'static str>>>>,
    pub fail_on: Option<(&'static str, RepositoryErrorCode)>,
    pub last_created: Option<(Task, TaskStateTransition, i64)>,
    pub last_saved: Option<(u64, Task, TaskStateTransition)>,
    pub last_terminated: Option<(u64, Task, TaskStateTransition)>,
}

impl FakeRepository {
    pub fn seed_task(&mut self, task: Task, history: Vec<TaskStateTransition>) {
        self.active_lease = task.state().requires_active_lease().then_some(ActiveLease {
            task_id: task.id(),
            acquired_at_ms: task.created_at_ms(),
        });
        self.transitions.insert(task.id(), history);
        self.tasks.insert(task.id(), task);
    }

    fn maybe_fail(&mut self, operation: &'static str) -> Result<(), RepositoryError> {
        if self.fail_on.is_some_and(|(target, _)| target == operation) {
            let (_, code) = self.fail_on.take().expect("matching failure exists");
            return Err(RepositoryError::with_source(
                code,
                std::io::Error::other("C:\\private\\secret.sqlite SELECT token"),
            ));
        }
        Ok(())
    }

    fn record(&mut self, operation: &'static str) {
        self.calls.push(operation);
        if let Some(calls) = &self.shared_calls {
            calls.lock().expect("call log lock").push(operation);
        }
    }
}

impl FoundationRepository for FakeRepository {
    fn create_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.record("create_task");
        self.maybe_fail("create_task")?;
        self.last_created = Some((
            task.clone(),
            initial_transition.clone(),
            lease_acquired_at_ms,
        ));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .insert(task.id(), vec![initial_transition.clone()]);
        self.active_lease = Some(ActiveLease {
            task_id: task.id(),
            acquired_at_ms: lease_acquired_at_ms,
        });
        Ok(())
    }

    fn get_task(&mut self, task_id: TaskId) -> Result<Option<Task>, RepositoryError> {
        self.record("get_task");
        self.maybe_fail("get_task")?;
        Ok(self.tasks.get(&task_id).cloned())
    }

    fn save_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.record("save_transition");
        self.maybe_fail("save_transition")?;
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        Ok(())
    }

    fn save_recovery_target(
        &mut self,
        _expected_version: u64,
        task: &Task,
    ) -> Result<(), RepositoryError> {
        self.record("save_recovery_target");
        self.maybe_fail("save_recovery_target")?;
        self.tasks.insert(task.id(), task.clone());
        Ok(())
    }

    fn terminate_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.record("terminate_task");
        self.maybe_fail("terminate_task")?;
        self.last_terminated = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        self.active_lease = None;
        Ok(())
    }

    fn list_task_transitions(
        &mut self,
        task_id: TaskId,
    ) -> Result<Vec<TaskStateTransition>, RepositoryError> {
        self.record("list_task_transitions");
        self.maybe_fail("list_task_transitions")?;
        Ok(self.transitions.get(&task_id).cloned().unwrap_or_default())
    }

    fn list_projects(&mut self) -> Result<Vec<ProjectSummary>, RepositoryError> {
        self.record("list_projects");
        self.maybe_fail("list_projects")?;
        Ok(self.projects.clone())
    }

    fn active_lease(&mut self) -> Result<Option<ActiveLease>, RepositoryError> {
        self.record("active_lease");
        self.maybe_fail("active_lease")?;
        Ok(self.active_lease)
    }
}

pub struct FakeTime {
    pub now: i64,
    pub failure: Option<PortFailure>,
    pub calls: usize,
}

impl FakeTime {
    pub fn at(now: i64) -> Self {
        Self {
            now,
            failure: None,
            calls: 0,
        }
    }
}

impl TimeProvider for FakeTime {
    fn now_ms(&mut self) -> Result<i64, PortFailure> {
        self.calls += 1;
        self.failure.map_or(Ok(self.now), Err)
    }
}

pub fn project(name: &str, root_path: &str, created_at_ms: i64) -> ProjectSummary {
    ProjectSummary {
        id: ProjectId::new(),
        name: name.to_owned(),
        root_path: root_path.to_owned(),
        created_at_ms,
        updated_at_ms: created_at_ms + 1,
    }
}

pub fn restored_task(
    state: TaskState,
    version: u64,
    updated_at_ms: i64,
    resume_target_state: Option<TaskState>,
) -> (Task, Vec<TaskStateTransition>) {
    let id = TaskId::new();
    let project_id = ProjectId::new();
    let created_at_ms = 10;
    let terminal_at_ms = (state.is_terminal() || state.is_post_terminal()).then_some(updated_at_ms);
    let task = Task::restore(TaskSnapshot {
        id,
        project_id,
        state,
        version,
        task_branch_identity: TaskBranchIdentity::for_task(id),
        resume_target_state,
        created_at_ms,
        updated_at_ms,
        terminal_at_ms,
    })
    .expect("test task must satisfy domain invariants");
    let initial = TaskStateTransition::initial(
        TaskStateTransitionId::new(),
        id,
        "test.actor".parse::<ActorKind>().expect("actor"),
        "test.reason".parse::<ReasonCode>().expect("reason"),
        created_at_ms,
    );
    (task, vec![initial])
}

pub fn storage_failure() -> PortFailure {
    PortFailure::new(FailureCategory::StorageUnavailable)
}
