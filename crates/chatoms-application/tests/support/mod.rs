#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chatoms_domain::{
    ActorKind, ProjectId, ReasonCode, Task, TaskBranchIdentity, TaskId, TaskSnapshot, TaskState,
    TaskStateTransition, TaskStateTransitionId, WorkKind,
};
use chatoms_ports::{
    TimeProvider,
    error::{FailureCategory, PortFailure},
    provider::ProviderKind,
    repository::{
        ActiveLease, FoundationRepository, GitInitApproval, GitOperationAttempt,
        GitOperationAttemptStatus, GitOperationKind, GitOperationReceipt, GitOperationReceiptKind,
        ProjectFilesystemIdentityRecord, ProjectRecord, ProjectSummary, ProviderConsent,
        RepositoryError, RepositoryErrorCode, TaskBriefRecord, TaskGitIsolation,
        TaskPlanningResultRecord,
    },
};

#[derive(Default)]
pub struct FakeRepository {
    pub projects: Vec<ProjectSummary>,
    pub project_records: HashMap<ProjectId, ProjectRecord>,
    pub project_identities: HashMap<ProjectId, ProjectFilesystemIdentityRecord>,
    pub tasks: HashMap<TaskId, Task>,
    pub transitions: HashMap<TaskId, Vec<TaskStateTransition>>,
    pub isolations: HashMap<TaskId, TaskGitIsolation>,
    pub briefs: HashMap<TaskId, TaskBriefRecord>,
    pub consents: HashMap<(TaskId, ProviderKind, WorkKind, u64), ProviderConsent>,
    pub planning_results: HashMap<TaskId, TaskPlanningResultRecord>,
    pub approvals: Vec<GitInitApproval>,
    pub attempts: HashMap<chatoms_domain::GitOperationId, GitOperationAttempt>,
    pub receipts: Vec<GitOperationReceipt>,
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
    fn create_project(&mut self, project: &ProjectRecord) -> Result<(), RepositoryError> {
        self.record("create_project");
        self.maybe_fail("create_project")?;
        if self
            .project_records
            .values()
            .any(|existing| existing.canonical_path_key == project.canonical_path_key)
        {
            return Err(RepositoryError::new(RepositoryErrorCode::DuplicateProject));
        }
        self.projects.push(ProjectSummary::from(project.clone()));
        self.project_records.insert(project.id, project.clone());
        Ok(())
    }

    fn create_project_with_identity(
        &mut self,
        project: &ProjectRecord,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.create_project(project)?;
        if self.project_identities.values().any(|existing| {
            existing.root_volume_serial_hex == identity.root_volume_serial_hex
                && existing.root_file_id_hex == identity.root_file_id_hex
        }) {
            return Err(RepositoryError::new(RepositoryErrorCode::DuplicateProject));
        }
        self.project_identities.insert(project.id, identity.clone());
        Ok(())
    }

    fn get_project_identity(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectFilesystemIdentityRecord>, RepositoryError> {
        self.record("get_project_identity");
        self.maybe_fail("get_project_identity")?;
        Ok(self.project_identities.get(&project_id).cloned())
    }

    fn update_project_identity(
        &mut self,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.record("update_project_identity");
        self.maybe_fail("update_project_identity")?;
        self.project_identities
            .insert(identity.project_id, identity.clone());
        Ok(())
    }

    fn get_project(
        &mut self,
        project_id: ProjectId,
    ) -> Result<Option<ProjectRecord>, RepositoryError> {
        self.record("get_project");
        self.maybe_fail("get_project")?;
        Ok(self.project_records.get(&project_id).cloned())
    }

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

    fn create_isolation_task(
        &mut self,
        task: &Task,
        initial_transition: &TaskStateTransition,
        classified_transition: &TaskStateTransition,
        lease_acquired_at_ms: i64,
        isolation: &TaskGitIsolation,
        brief: Option<&TaskBriefRecord>,
    ) -> Result<(), RepositoryError> {
        self.record("create_isolation_task");
        self.maybe_fail("create_isolation_task")?;
        self.tasks.insert(task.id(), task.clone());
        self.transitions.insert(
            task.id(),
            vec![initial_transition.clone(), classified_transition.clone()],
        );
        self.isolations.insert(task.id(), isolation.clone());
        if let Some(brief) = brief {
            self.briefs.insert(task.id(), brief.clone());
        }
        self.active_lease = Some(ActiveLease {
            task_id: task.id(),
            acquired_at_ms: lease_acquired_at_ms,
        });
        Ok(())
    }

    fn get_task_isolation(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskGitIsolation>, RepositoryError> {
        self.record("get_task_isolation");
        self.maybe_fail("get_task_isolation")?;
        Ok(self.isolations.get(&task_id).cloned())
    }

    fn get_task_brief(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskBriefRecord>, RepositoryError> {
        self.record("get_task_brief");
        self.maybe_fail("get_task_brief")?;
        Ok(self.briefs.get(&task_id).cloned())
    }

    fn get_task_planning_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskPlanningResultRecord>, RepositoryError> {
        self.record("get_task_planning_result");
        self.maybe_fail("get_task_planning_result")?;
        Ok(self.planning_results.get(&task_id).cloned())
    }

    fn get_provider_consent(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        self.record("get_provider_consent");
        self.maybe_fail("get_provider_consent")?;
        Ok(self
            .consents
            .get(&(task_id, provider, work_kind, approved_task_version))
            .copied())
    }

    fn save_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        self.record("save_planning_transition");
        self.maybe_fail("save_planning_transition")?;
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        if let Some(consent) = consent {
            self.consents.insert(
                (
                    consent.task_id,
                    consent.provider,
                    consent.work_kind,
                    consent.approved_task_version,
                ),
                *consent,
            );
        }
        Ok(())
    }

    fn save_planning_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskPlanningResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        self.record("save_planning_result");
        self.maybe_fail("save_planning_result")?;
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        self.planning_results.insert(task.id(), result.clone());
        if terminal {
            self.active_lease = None;
        }
        Ok(())
    }

    fn begin_git_initialization(
        &mut self,
        _expected_version: u64,
        isolation: &TaskGitIsolation,
        approval: &GitInitApproval,
    ) -> Result<(), RepositoryError> {
        self.record("begin_git_initialization");
        self.maybe_fail("begin_git_initialization")?;
        self.isolations.insert(isolation.task_id, isolation.clone());
        self.approvals.push(*approval);
        self.attempts.insert(
            approval.operation_id,
            GitOperationAttempt {
                operation_id: approval.operation_id,
                task_id: approval.task_id,
                project_id: approval.project_id,
                operation_kind: GitOperationKind::GitInitialize,
                status: GitOperationAttemptStatus::IntentRecorded,
                approved_task_version: approval.approved_task_version,
                project_identity_revision: self
                    .project_identities
                    .get(&approval.project_id)
                    .map_or(1, |identity| identity.revision),
                created_at_ms: approval.approved_at_ms,
                updated_at_ms: approval.approved_at_ms,
            },
        );
        Ok(())
    }

    fn save_isolation_intent(
        &mut self,
        _expected_version: u64,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.record("save_isolation_intent");
        self.maybe_fail("save_isolation_intent")?;
        self.isolations.insert(isolation.task_id, isolation.clone());
        Ok(())
    }

    fn append_git_operation_receipt(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
        kind: GitOperationReceiptKind,
        evidence: Option<&str>,
        recorded_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.record("append_git_operation_receipt");
        self.maybe_fail("append_git_operation_receipt")?;
        self.receipts.push(GitOperationReceipt {
            operation_id,
            sequence: u64::try_from(
                self.receipts
                    .iter()
                    .filter(|receipt| receipt.operation_id == operation_id)
                    .count()
                    + 1,
            )
            .expect("receipt count"),
            kind,
            evidence: evidence.map(ToOwned::to_owned),
            recorded_at_ms,
        });
        Ok(())
    }

    fn list_git_operation_receipts(
        &mut self,
        operation_id: chatoms_domain::GitOperationId,
    ) -> Result<Vec<GitOperationReceipt>, RepositoryError> {
        Ok(self
            .receipts
            .iter()
            .filter(|receipt| receipt.operation_id == operation_id)
            .cloned()
            .collect())
    }

    fn list_incomplete_git_operations(
        &mut self,
    ) -> Result<Vec<GitOperationAttempt>, RepositoryError> {
        self.record("list_incomplete_git_operations");
        self.maybe_fail("list_incomplete_git_operations")?;
        Ok(self
            .attempts
            .values()
            .filter(|attempt| attempt.status == GitOperationAttemptStatus::IntentRecorded)
            .copied()
            .collect())
    }

    fn save_isolation_transition(
        &mut self,
        _expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.record("save_isolation_transition");
        self.maybe_fail("save_isolation_transition")?;
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        self.isolations.insert(task.id(), isolation.clone());
        if isolation.status == chatoms_ports::repository::GitIsolationStatus::WorktreeCreating {
            let operation_id = isolation.operation_id.expect("worktree operation id");
            self.attempts.insert(
                operation_id,
                GitOperationAttempt {
                    operation_id,
                    task_id: task.id(),
                    project_id: task.project_id(),
                    operation_kind: GitOperationKind::WorktreeCreate,
                    status: GitOperationAttemptStatus::IntentRecorded,
                    approved_task_version: isolation.expected_task_version,
                    project_identity_revision: self
                        .project_identities
                        .get(&task.project_id())
                        .map_or(1, |identity| identity.revision),
                    created_at_ms: isolation.updated_at_ms,
                    updated_at_ms: isolation.updated_at_ms,
                },
            );
        }
        if isolation.status == chatoms_ports::repository::GitIsolationStatus::RecoveryRequired
            && let Some(operation_id) = isolation.operation_id
        {
            if let Some(attempt) = self.attempts.get_mut(&operation_id) {
                attempt.status = GitOperationAttemptStatus::RecoveryRequired;
            }
            let sequence = u64::try_from(
                self.receipts
                    .iter()
                    .filter(|receipt| receipt.operation_id == operation_id)
                    .count()
                    + 1,
            )
            .expect("receipt count");
            self.receipts.push(GitOperationReceipt {
                operation_id,
                sequence,
                kind: GitOperationReceiptKind::RecoveryRequired,
                evidence: None,
                recorded_at_ms: isolation.updated_at_ms,
            });
        }
        Ok(())
    }

    fn save_git_initialization_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
        identity: &ProjectFilesystemIdentityRecord,
    ) -> Result<(), RepositoryError> {
        self.record("save_git_initialization_completion");
        self.maybe_fail("save_git_initialization_completion")?;
        self.save_isolation_transition(expected_version, task, transition, isolation)?;
        self.project_identities
            .insert(identity.project_id, identity.clone());
        if let Some(operation_id) = isolation.operation_id {
            if let Some(attempt) = self.attempts.get_mut(&operation_id) {
                attempt.status = GitOperationAttemptStatus::Completed;
            }
            self.receipts.push(GitOperationReceipt {
                operation_id,
                sequence: u64::try_from(self.receipts.len() + 1).expect("receipt count"),
                kind: GitOperationReceiptKind::CompletionRecorded,
                evidence: None,
                recorded_at_ms: isolation.updated_at_ms,
            });
        }
        Ok(())
    }

    fn save_worktree_completion(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.record("save_worktree_completion");
        self.maybe_fail("save_worktree_completion")?;
        self.save_isolation_transition(expected_version, task, transition, isolation)?;
        if let Some(operation_id) = isolation.operation_id {
            if let Some(attempt) = self.attempts.get_mut(&operation_id) {
                attempt.status = GitOperationAttemptStatus::Completed;
            }
            self.receipts.push(GitOperationReceipt {
                operation_id,
                sequence: u64::try_from(self.receipts.len() + 1).expect("receipt count"),
                kind: GitOperationReceiptKind::CompletionRecorded,
                evidence: None,
                recorded_at_ms: isolation.updated_at_ms,
            });
        }
        Ok(())
    }

    fn terminate_isolation_task(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        isolation: &TaskGitIsolation,
    ) -> Result<(), RepositoryError> {
        self.save_isolation_transition(expected_version, task, transition, isolation)?;
        self.active_lease = None;
        Ok(())
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
        canonical_path_key: root_path.to_lowercase().replace('\\', "/"),
        display_path: root_path.to_owned(),
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
