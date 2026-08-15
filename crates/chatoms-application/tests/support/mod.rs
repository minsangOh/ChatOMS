#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chatoms_domain::{
    ActorKind, ContextDataScope, HighRiskCategory, ProjectId, ReasonCode, Task, TaskBranchIdentity,
    TaskId, TaskSnapshot, TaskState, TaskStateTransition, TaskStateTransitionId,
    ValidationCommandKind, WorkKind,
};
use chatoms_ports::{
    TimeProvider,
    diff::DiffContentHash,
    error::{FailureCategory, PortFailure},
    provider::ProviderKind,
    repository::{
        ActiveLease, ContextPackageManifestRecord, ContextPackagePreparation, DiffApprovalRecord,
        FoundationRepository, GitInitApproval, GitIsolationStatus, GitOperationAttempt,
        GitOperationAttemptStatus, GitOperationKind, GitOperationReceipt, GitOperationReceiptKind,
        HighRiskApprovalRecord, ProjectFilesystemIdentityRecord, ProjectRecord, ProjectSummary,
        ProviderConsent, RepositoryError, RepositoryErrorCode, TaskBriefRecord, TaskGitIsolation,
        TaskImplementationResultRecord, TaskPlanningResultRecord, TaskReviewResultRecord,
        ValidationCommandApprovalRecord, ValidationCommandResultAttempt,
        ValidationCommandResultRecord,
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
    pub consents: HashMap<(TaskId, ProviderKind, WorkKind, u64, ContextDataScope), ProviderConsent>,
    pub context_package_manifests: HashMap<
        (TaskId, ProviderKind, WorkKind, u64, ContextDataScope),
        ContextPackageManifestRecord,
    >,
    pub planning_results: HashMap<TaskId, TaskPlanningResultRecord>,
    pub implementation_results: HashMap<TaskId, TaskImplementationResultRecord>,
    pub review_results: HashMap<TaskId, TaskReviewResultRecord>,
    pub validation_command_approvals:
        HashMap<(TaskId, u64, ValidationCommandKind), ValidationCommandApprovalRecord>,
    pub validation_command_results: Vec<ValidationCommandResultRecord>,
    pub high_risk_approvals: HashMap<(TaskId, u64, HighRiskCategory), HighRiskApprovalRecord>,
    pub diff_approvals: HashMap<(TaskId, u64, DiffContentHash), DiffApprovalRecord>,
    pub approvals: Vec<GitInitApproval>,
    pub attempts: HashMap<chatoms_domain::GitOperationId, GitOperationAttempt>,
    pub receipts: Vec<GitOperationReceipt>,
    pub active_lease: Option<ActiveLease>,
    pub calls: Vec<&'static str>,
    pub shared_calls: Option<Arc<Mutex<Vec<&'static str>>>>,
    pub fail_on: Option<(&'static str, RepositoryErrorCode)>,
    /// Independent of `fail_on`: makes the *next* `save_transition` call fail
    /// once, regardless of whether `fail_on` has already fired. Lets a test
    /// simulate a primary write failure whose `RecoveryRequired` fallback
    /// (which itself writes via `save_transition`) also fails, since
    /// `fail_on` alone is single-shot and consumed by the primary failure.
    pub fail_save_transition_once: Option<RepositoryErrorCode>,
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
        if operation == "save_transition"
            && let Some(code) = self.fail_save_transition_once.take()
        {
            return Err(RepositoryError::with_source(
                code,
                std::io::Error::other("C:\\private\\secret.sqlite SELECT token"),
            ));
        }
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

    /// Shared core of the three `prepare_*_context_package` fakes, mirroring
    /// `SqliteFoundationRepository`'s `prepare_context_package` helper:
    /// looks up the exact `(task_id, Claude, work_kind, expected_version,
    /// ContextPackageV1)` consent and manifest together, and either reuses
    /// both unchanged, inserts both fresh, or fails closed as
    /// `InvalidPersistenceState` if exactly one of the pair exists.
    fn fake_prepare_context_package(
        &mut self,
        task_id: TaskId,
        work_kind: WorkKind,
        expected_version: u64,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        let key = (
            task_id,
            ProviderKind::Claude,
            work_kind,
            expected_version,
            ContextDataScope::ContextPackageV1,
        );
        let existing_consent = self.consents.get(&key).copied();
        let existing_manifest = self.context_package_manifests.get(&key).copied();
        match (existing_consent, existing_manifest) {
            (Some(consent), Some(manifest)) => Ok(ContextPackagePreparation { consent, manifest }),
            (None, None) => {
                let consent = ProviderConsent {
                    task_id,
                    provider: ProviderKind::Claude,
                    work_kind,
                    approved_task_version: expected_version,
                    data_scope: ContextDataScope::ContextPackageV1,
                    consented_at_ms: prepared_at_ms,
                };
                self.consents.insert(key, consent);
                let manifest = ContextPackageManifestRecord {
                    task_id,
                    provider: ProviderKind::Claude,
                    work_kind,
                    approved_task_version: expected_version,
                    data_scope: ContextDataScope::ContextPackageV1,
                    created_at_ms: prepared_at_ms,
                };
                self.context_package_manifests.insert(key, manifest);
                Ok(ContextPackagePreparation { consent, manifest })
            }
            (Some(_), None) | (None, Some(_)) => Err(RepositoryError::new(
                RepositoryErrorCode::InvalidPersistenceState,
            )),
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

    fn get_task_implementation_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskImplementationResultRecord>, RepositoryError> {
        self.record("get_task_implementation_result");
        self.maybe_fail("get_task_implementation_result")?;
        Ok(self.implementation_results.get(&task_id).cloned())
    }

    fn get_task_review_result(
        &mut self,
        task_id: TaskId,
    ) -> Result<Option<TaskReviewResultRecord>, RepositoryError> {
        self.record("get_task_review_result");
        self.maybe_fail("get_task_review_result")?;
        Ok(self.review_results.get(&task_id).cloned())
    }

    fn get_provider_consent(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ProviderConsent>, RepositoryError> {
        self.record("get_provider_consent");
        self.maybe_fail("get_provider_consent")?;
        Ok(self
            .consents
            .get(&(
                task_id,
                provider,
                work_kind,
                approved_task_version,
                data_scope,
            ))
            .copied())
    }

    fn get_context_package_manifest(
        &mut self,
        task_id: TaskId,
        provider: ProviderKind,
        work_kind: WorkKind,
        approved_task_version: u64,
        data_scope: ContextDataScope,
    ) -> Result<Option<ContextPackageManifestRecord>, RepositoryError> {
        self.record("get_context_package_manifest");
        self.maybe_fail("get_context_package_manifest")?;
        Ok(self
            .context_package_manifests
            .get(&(
                task_id,
                provider,
                work_kind,
                approved_task_version,
                data_scope,
            ))
            .copied())
    }

    fn get_high_risk_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        risk_category: HighRiskCategory,
    ) -> Result<Option<HighRiskApprovalRecord>, RepositoryError> {
        self.record("get_high_risk_approval");
        self.maybe_fail("get_high_risk_approval")?;
        Ok(self
            .high_risk_approvals
            .get(&(task_id, approved_task_version, risk_category))
            .copied())
    }

    fn ensure_high_risk_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        risk_category: HighRiskCategory,
        approved_at_ms: i64,
    ) -> Result<HighRiskApprovalRecord, RepositoryError> {
        self.record("ensure_high_risk_approval");
        self.maybe_fail("ensure_high_risk_approval")?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        let key = (task_id, expected_version, risk_category);
        if let Some(existing) = self.high_risk_approvals.get(&key) {
            return Ok(*existing);
        }
        let approval = HighRiskApprovalRecord {
            task_id,
            approved_task_version: expected_version,
            risk_category,
            approved_at_ms,
        };
        self.high_risk_approvals.insert(key, approval);
        Ok(approval)
    }

    fn save_diff_approval(&mut self, approval: &DiffApprovalRecord) -> Result<(), RepositoryError> {
        self.record("save_diff_approval");
        self.maybe_fail("save_diff_approval")?;
        let key = (
            approval.task_id,
            approval.approved_task_version,
            approval.diff_content_hash,
        );
        if self.diff_approvals.contains_key(&key) {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        self.diff_approvals.insert(key, *approval);
        Ok(())
    }

    fn get_diff_approval(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        diff_content_hash: DiffContentHash,
    ) -> Result<Option<DiffApprovalRecord>, RepositoryError> {
        self.record("get_diff_approval");
        self.maybe_fail("get_diff_approval")?;
        Ok(self
            .diff_approvals
            .get(&(task_id, approved_task_version, diff_content_hash))
            .copied())
    }

    fn ensure_diff_approval(
        &mut self,
        task_id: TaskId,
        expected_version: u64,
        diff_content_hash: DiffContentHash,
        approved_at_ms: i64,
    ) -> Result<DiffApprovalRecord, RepositoryError> {
        self.record("ensure_diff_approval");
        self.maybe_fail("ensure_diff_approval")?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        let key = (task_id, expected_version, diff_content_hash);
        if let Some(existing) = self.diff_approvals.get(&key) {
            return Ok(*existing);
        }
        let approval = DiffApprovalRecord {
            task_id,
            approved_task_version: expected_version,
            diff_content_hash,
            approved_at_ms,
        };
        self.diff_approvals.insert(key, approval);
        Ok(approval)
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
                    consent.data_scope,
                ),
                *consent,
            );
        }
        Ok(())
    }

    fn save_context_package_planning_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.record("save_context_package_planning_transition");
        self.maybe_fail("save_context_package_planning_transition")?;
        if task.state() != TaskState::Planning {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let current = self
            .tasks
            .get(&task.id())
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::WorktreeReady {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let isolation = self
            .isolations
            .get(&task.id())
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::IsolationNotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let key = (
            task.id(),
            ProviderKind::Claude,
            WorkKind::Planning,
            expected_version,
            ContextDataScope::ContextPackageV1,
        );
        let consent = self.consents.get(&key).copied();
        let manifest = self.context_package_manifests.get(&key).copied();
        match (consent, manifest) {
            (Some(_), Some(_)) => {}
            (None, None) => {
                return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
            }
            (Some(_), None) | (None, Some(_)) => {
                return Err(RepositoryError::new(
                    RepositoryErrorCode::InvalidPersistenceState,
                ));
            }
        }
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        Ok(())
    }

    fn save_context_package_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
    ) -> Result<(), RepositoryError> {
        self.record("save_context_package_implementation_transition");
        self.maybe_fail("save_context_package_implementation_transition")?;
        if task.state() != TaskState::Implementing {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let current = self
            .tasks
            .get(&task.id())
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if current.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if current.state() != TaskState::AwaitingDesignApproval {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let isolation = self
            .isolations
            .get(&task.id())
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::IsolationNotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let plan_ready = self.planning_results.get(&task.id()).is_some_and(|result| {
            result.outcome == chatoms_ports::repository::PlanningResultOutcome::Completed
                && result
                    .plan_text
                    .as_deref()
                    .is_some_and(|text| !text.is_empty())
        });
        if !plan_ready {
            return Err(RepositoryError::new(
                RepositoryErrorCode::InvalidPersistenceState,
            ));
        }
        let key = (
            task.id(),
            ProviderKind::Claude,
            WorkKind::Implementation,
            expected_version,
            ContextDataScope::ContextPackageV1,
        );
        let consent = self.consents.get(&key).copied();
        let manifest = self.context_package_manifests.get(&key).copied();
        match (consent, manifest) {
            (Some(_), Some(_)) => {}
            (None, None) => {
                return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
            }
            (Some(_), None) | (None, Some(_)) => {
                return Err(RepositoryError::new(
                    RepositoryErrorCode::InvalidPersistenceState,
                ));
            }
        }
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        Ok(())
    }

    fn save_implementation_transition(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        consent: Option<&ProviderConsent>,
    ) -> Result<(), RepositoryError> {
        self.record("save_implementation_transition");
        self.maybe_fail("save_implementation_transition")?;
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
                    consent.data_scope,
                ),
                *consent,
            );
        }
        Ok(())
    }

    fn save_review_consent(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        data_scope: ContextDataScope,
        consented_at_ms: i64,
    ) -> Result<ProviderConsent, RepositoryError> {
        self.record("save_review_consent");
        self.maybe_fail("save_review_consent")?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if task.state() != TaskState::Reviewing {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let key = (
            task_id,
            ProviderKind::Claude,
            WorkKind::Review,
            expected_version,
            data_scope,
        );
        if let Some(existing) = self.consents.get(&key) {
            return Ok(*existing);
        }
        let consent = ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Review,
            approved_task_version: expected_version,
            data_scope,
            consented_at_ms,
        };
        self.consents.insert(key, consent);
        Ok(consent)
    }

    fn prepare_planning_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.record("prepare_planning_context_package");
        self.maybe_fail("prepare_planning_context_package")?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if task.state() != TaskState::WorktreeReady {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let isolation = self
            .isolations
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::IsolationNotFound))?;
        if isolation.status != GitIsolationStatus::WorktreeReady {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        self.fake_prepare_context_package(
            task_id,
            WorkKind::Planning,
            expected_version,
            prepared_at_ms,
        )
    }

    fn prepare_implementation_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.record("prepare_implementation_context_package");
        self.maybe_fail("prepare_implementation_context_package")?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if task.state() != TaskState::AwaitingDesignApproval {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        self.fake_prepare_context_package(
            task_id,
            WorkKind::Implementation,
            expected_version,
            prepared_at_ms,
        )
    }

    fn prepare_review_context_package(
        &mut self,
        expected_version: u64,
        task_id: TaskId,
        prepared_at_ms: i64,
    ) -> Result<ContextPackagePreparation, RepositoryError> {
        self.record("prepare_review_context_package");
        self.maybe_fail("prepare_review_context_package")?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| RepositoryError::new(RepositoryErrorCode::TaskNotFound))?;
        if task.version() != expected_version {
            return Err(RepositoryError::new(RepositoryErrorCode::VersionConflict));
        }
        if task.state() != TaskState::Reviewing {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        self.fake_prepare_context_package(
            task_id,
            WorkKind::Review,
            expected_version,
            prepared_at_ms,
        )
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

    fn save_implementation_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskImplementationResultRecord,
    ) -> Result<(), RepositoryError> {
        self.record("save_implementation_result");
        self.maybe_fail("save_implementation_result")?;
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        self.implementation_results
            .insert(task.id(), result.clone());
        Ok(())
    }

    fn save_review_result(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        result: &TaskReviewResultRecord,
        terminal: bool,
    ) -> Result<(), RepositoryError> {
        self.record("save_review_result");
        self.maybe_fail("save_review_result")?;
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
        self.review_results.insert(task.id(), result.clone());
        if terminal {
            self.active_lease = None;
        }
        Ok(())
    }

    fn save_validation_command_approval(
        &mut self,
        approval: &ValidationCommandApprovalRecord,
    ) -> Result<(), RepositoryError> {
        self.record("save_validation_command_approval");
        self.maybe_fail("save_validation_command_approval")?;
        let key = (
            approval.task_id,
            approval.approved_task_version,
            approval.kind,
        );
        if self.validation_command_approvals.contains_key(&key) {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        self.validation_command_approvals
            .insert(key, approval.clone());
        Ok(())
    }

    fn list_validation_command_approvals(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
    ) -> Result<Vec<ValidationCommandApprovalRecord>, RepositoryError> {
        self.record("list_validation_command_approvals");
        self.maybe_fail("list_validation_command_approvals")?;
        let mut approvals: Vec<_> = self
            .validation_command_approvals
            .iter()
            .filter(|((id, version, _), _)| *id == task_id && *version == approved_task_version)
            .map(|(_, approval)| approval.clone())
            .collect();
        approvals.sort_by_key(|approval| {
            ValidationCommandKind::ALL
                .iter()
                .position(|kind| *kind == approval.kind)
                .unwrap_or(usize::MAX)
        });
        Ok(approvals)
    }

    fn append_validation_command_result(
        &mut self,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<ValidationCommandResultRecord, RepositoryError> {
        self.record("append_validation_command_result");
        self.maybe_fail("append_validation_command_result")?;
        let key = (attempt.task_id, attempt.approved_task_version, attempt.kind);
        if !self.validation_command_approvals.contains_key(&key) {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let attempt_sequence = u32::try_from(
            self.validation_command_results
                .iter()
                .filter(|existing| {
                    existing.task_id == attempt.task_id
                        && existing.approved_task_version == attempt.approved_task_version
                        && existing.kind == attempt.kind
                })
                .count(),
        )
        .expect("attempt count fits u32 in tests")
            + 1;
        let record = ValidationCommandResultRecord {
            task_id: attempt.task_id,
            approved_task_version: attempt.approved_task_version,
            kind: attempt.kind,
            attempt_sequence,
            outcome: attempt.outcome,
            exit_code: attempt.exit_code,
            safe_summary: attempt.safe_summary.clone(),
            started_at_ms: attempt.started_at_ms,
            completed_at_ms: attempt.completed_at_ms,
        };
        self.validation_command_results.push(record.clone());
        Ok(record)
    }

    fn list_validation_command_results(
        &mut self,
        task_id: TaskId,
        approved_task_version: u64,
        kind: ValidationCommandKind,
    ) -> Result<Vec<ValidationCommandResultRecord>, RepositoryError> {
        self.record("list_validation_command_results");
        self.maybe_fail("list_validation_command_results")?;
        let mut results: Vec<_> = self
            .validation_command_results
            .iter()
            .filter(|record| {
                record.task_id == task_id
                    && record.approved_task_version == approved_task_version
                    && record.kind == kind
            })
            .cloned()
            .collect();
        results.sort_by_key(|record| record.attempt_sequence);
        Ok(results)
    }

    fn finalize_validation_command_batch(
        &mut self,
        expected_version: u64,
        task: &Task,
        transition: &TaskStateTransition,
        attempt: &ValidationCommandResultAttempt,
    ) -> Result<(), RepositoryError> {
        self.record("finalize_validation_command_batch");
        self.maybe_fail("finalize_validation_command_batch")?;
        let key = (attempt.task_id, attempt.approved_task_version, attempt.kind);
        if !self.validation_command_approvals.contains_key(&key) {
            return Err(RepositoryError::new(RepositoryErrorCode::InvalidAggregate));
        }
        let attempt_sequence = u32::try_from(
            self.validation_command_results
                .iter()
                .filter(|existing| {
                    existing.task_id == attempt.task_id
                        && existing.approved_task_version == attempt.approved_task_version
                        && existing.kind == attempt.kind
                })
                .count(),
        )
        .expect("attempt count fits u32 in tests")
            + 1;
        self.validation_command_results
            .push(ValidationCommandResultRecord {
                task_id: attempt.task_id,
                approved_task_version: attempt.approved_task_version,
                kind: attempt.kind,
                attempt_sequence,
                outcome: attempt.outcome,
                exit_code: attempt.exit_code,
                safe_summary: attempt.safe_summary.clone(),
                started_at_ms: attempt.started_at_ms,
                completed_at_ms: attempt.completed_at_ms,
            });
        self.last_saved = Some((expected_version, task.clone(), transition.clone()));
        self.tasks.insert(task.id(), task.clone());
        self.transitions
            .entry(task.id())
            .or_default()
            .push(transition.clone());
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
