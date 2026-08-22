#[allow(dead_code)]
#[path = "operation_risk_declaration/support.rs"]
mod operation_risk_support;
mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    error::ApplicationErrorCode,
    implementation_execution::{
        BeginImplementationExecutionRequest, ImplementationExecutionRecorder,
        ImplementationExecutionStarter,
    },
    operation_risk_declaration::{
        DeclareProviderImplementationRiskRequest, OperationRiskDeclarationService,
    },
};
use chatoms_domain::{
    ContextDataScope, GitOperationId, HighRiskCategory, OperationRiskKind, ProjectId, TaskId,
    TaskState, WorkKind,
};
use chatoms_ports::{
    error::{FailureCategory, PortFailure},
    git::RepositoryKind,
    implementation::{
        ImplementationExecutionBrief, ImplementationExecutionResult,
        ImplementationExecutionStartOutcome, PolicyGatedClaudeImplementationExecutor,
    },
    process::{AtomicCancellationSignal, CancellationSignal},
    provider::{
        ProviderCapabilities, ProviderCapabilityPort, ProviderCapabilityStatus, ProviderKind,
    },
    repository::{
        FoundationRepository, GitIsolationStatus, HighRiskApprovalRecord,
        ImplementationResultOutcome, PlanningResultOutcome, ProjectFilesystemIdentityRecord,
        ProjectRecord, RepositoryErrorCode, TaskBriefRecord, TaskGitIsolation,
        TaskPlanningResultRecord,
    },
};

use operation_risk_support::FakeFilesystem;
use support::{FakeRepository, FakeTime, restored_task};

struct FakeCapability(ProviderCapabilityStatus);

impl ProviderCapabilityPort for FakeCapability {
    fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure> {
        Ok(ProviderCapabilities {
            claude: self.0,
            codex: ProviderCapabilityStatus::Unsupported,
        })
    }
}

struct UnexpectedCapability;

impl ProviderCapabilityPort for UnexpectedCapability {
    fn provider_capabilities(&mut self) -> Result<ProviderCapabilities, PortFailure> {
        panic!("policy rejection must precede every provider capability probe")
    }
}

type ObservedRun = (PathBuf, String, String, String, String);

/// Deliberately looks like a leaked credential so panic-containment tests
/// can assert this exact string never survives into anything the caught
/// panic's containment path returns, records, or renders.
const PANIC_SENTINEL: &str = "SIMULATED_EXECUTOR_PANIC_must_never_leak_sk-fake0000000000";

struct ScriptedExecutor {
    scripted: Option<Result<ImplementationExecutionStartOutcome, ()>>,
    observed: Vec<ObservedRun>,
    panics: bool,
}

impl ScriptedExecutor {
    fn completed(outcome: ImplementationResultOutcome) -> Self {
        Self {
            scripted: Some(Ok(ImplementationExecutionStartOutcome::Completed(
                ImplementationExecutionResult {
                    outcome,
                    exit_code: Some(0),
                    turn_count: Some(1),
                },
            ))),
            observed: Vec::new(),
            panics: false,
        }
    }

    fn preflight_rejected() -> Self {
        Self {
            scripted: Some(Ok(ImplementationExecutionStartOutcome::PreflightRejected)),
            observed: Vec::new(),
            panics: false,
        }
    }

    fn failing() -> Self {
        Self {
            scripted: Some(Err(())),
            observed: Vec::new(),
            panics: false,
        }
    }

    /// Simulates a genuine Rust panic inside the executor (e.g. an
    /// unexpected crash deep in adapter/process-runner code), rather than
    /// an ordinary `Err`.
    fn panicking() -> Self {
        Self {
            scripted: None,
            observed: Vec::new(),
            panics: true,
        }
    }
}

impl PolicyGatedClaudeImplementationExecutor for ScriptedExecutor {
    fn start_implementation(
        &mut self,
        _task_id: TaskId,
        _started_task_version: u64,
        worktree: &Path,
        brief: ImplementationExecutionBrief<'_>,
        _cancellation: &dyn CancellationSignal,
    ) -> Result<ImplementationExecutionStartOutcome, PortFailure> {
        self.observed.push((
            worktree.to_path_buf(),
            brief.requirements.to_owned(),
            brief.completion_criteria.to_owned(),
            brief.prohibited_scope.to_owned(),
            brief.plan_text.to_owned(),
        ));
        if self.panics {
            panic!("{PANIC_SENTINEL}");
        }
        match self.scripted.take() {
            Some(Ok(outcome)) => Ok(outcome),
            Some(Err(())) | None => Err(PortFailure::new(FailureCategory::Unsupported)),
        }
    }
}

fn worktree_ready_isolation(task_id: TaskId, expected_version: u64) -> TaskGitIsolation {
    TaskGitIsolation {
        task_id,
        project_id: ProjectId::new(),
        status: GitIsolationStatus::WorktreeReady,
        operation_id: Some(GitOperationId::new()),
        expected_task_version: expected_version,
        base_branch: Some("main".to_owned()),
        base_commit: Some("a".repeat(40)),
        worktree_path: Some("C:/managed/task".to_owned()),
        branch_created_by_app: true,
        worktree_created_by_app: true,
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

fn brief_record(task_id: TaskId) -> TaskBriefRecord {
    TaskBriefRecord {
        task_id,
        requirements: "Add CSV export".to_owned(),
        completion_criteria: "Export button downloads a CSV".to_owned(),
        prohibited_scope: "Do not touch the import pipeline".to_owned(),
        created_at_ms: 10,
    }
}

fn completed_planning_result(task_id: TaskId, plan_text: &str) -> TaskPlanningResultRecord {
    TaskPlanningResultRecord {
        task_id,
        provider: ProviderKind::Claude,
        work_kind: WorkKind::Planning,
        outcome: PlanningResultOutcome::Completed,
        exit_code: Some(0),
        turn_count: Some(3),
        started_at_ms: 5,
        completed_at_ms: 20,
        plan_text: Some(plan_text.to_owned()),
    }
}

/// A task ready to start Implementation: `AwaitingDesignApproval` state
/// with a matching `WorktreeReady` isolation record, an attached brief, and
/// a `Completed` Claude Planning result carrying plan text.
fn setup_awaiting_design_approval_without_declaration(
    version: u64,
) -> (FakeRepository, FakeFilesystem, TaskId) {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    let mut isolation = worktree_ready_isolation(task_id, version);
    isolation.project_id = task.project_id();
    repository.isolations.insert(task_id, isolation);
    repository.briefs.insert(task_id, brief_record(task_id));
    repository
        .planning_results
        .insert(task_id, completed_planning_result(task_id, "masked plan"));
    repository.project_records.insert(
        task.project_id(),
        ProjectRecord {
            id: task.project_id(),
            name: "Project".to_owned(),
            root_path: "C:/project".to_owned(),
            canonical_path_key: "c:/project".to_owned(),
            display_path: "Project".to_owned(),
            created_at_ms: 10,
            updated_at_ms: 20,
        },
    );
    repository.project_identities.insert(
        task.project_id(),
        ProjectFilesystemIdentityRecord {
            project_id: task.project_id(),
            root_volume_serial_hex: "0000000000000001".to_owned(),
            root_file_id_hex: "11111111111111111111111111111111".to_owned(),
            repository_kind: RepositoryKind::Git,
            git_common_volume_serial_hex: None,
            git_common_file_id_hex: None,
            confirmed: true,
            revision: 1,
            verified_at_ms: 20,
        },
    );
    repository.seed_task(task, history);
    let filesystem = FakeFilesystem {
        identities: std::collections::HashMap::from([
            (
                PathBuf::from("C:/project"),
                chatoms_ports::filesystem::DirectoryIdentity {
                    canonical_path: PathBuf::from("C:/project"),
                    volume_serial_hex: "0000000000000001".to_owned(),
                    file_id_hex: "11111111111111111111111111111111".to_owned(),
                },
            ),
            (
                PathBuf::from("C:/managed/task"),
                chatoms_ports::filesystem::DirectoryIdentity {
                    canonical_path: PathBuf::from("C:/managed/task"),
                    volume_serial_hex: "0000000000000002".to_owned(),
                    file_id_hex: "22222222222222222222222222222222".to_owned(),
                },
            ),
        ]),
        failures: std::collections::HashMap::new(),
    };
    (repository, filesystem, task_id)
}

fn setup_awaiting_design_approval(version: u64) -> (FakeRepository, FakeFilesystem, TaskId) {
    let (mut repository, mut filesystem, task_id) =
        setup_awaiting_design_approval_without_declaration(version);
    OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(DeclareProviderImplementationRiskRequest {
            task_id,
            expected_version: version,
            risk_categories: Vec::new(),
            declared_at_ms: 25,
        })
        .expect("declare explicit empty provider implementation risk");
    (repository, filesystem, task_id)
}

/// A task already in `Implementing`, as `run_and_record` expects to find
/// it.
fn setup_implementing(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Implementing, version, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    (repository, task_id)
}

#[test]
fn begin_transitions_to_implementing_and_returns_worktree_brief_and_plan_when_supported() {
    let (mut repository, mut filesystem, task_id) = setup_awaiting_design_approval(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let inputs = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .expect("begin succeeds");

    assert_eq!(inputs.task.state, TaskState::Implementing);
    assert_eq!(inputs.task.version, 4);
    assert_eq!(inputs.worktree_path, "C:/managed/task");
    assert_eq!(inputs.brief.requirements, "Add CSV export");
    assert_eq!(inputs.plan_text, "masked plan");
}

#[test]
fn begin_rejects_when_provider_implementation_assessment_is_missing() {
    let (mut repository, mut filesystem, task_id) =
        setup_awaiting_design_approval_without_declaration(3);
    let mut time = FakeTime::at(30);
    let mut capability = UnexpectedCapability;

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("a missing operation-risk declaration must fail closed");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert_eq!(repository.tasks[&task_id].version(), 3);
    assert!(repository.last_saved.is_none());
    assert!(
        repository
            .get_provider_consent(
                task_id,
                ProviderKind::Claude,
                WorkKind::Implementation,
                3,
                ContextDataScope::LegacyPhase4,
            )
            .expect("consent lookup")
            .is_none()
    );
}

#[test]
fn begin_accepts_non_empty_assessment_with_exact_approval() {
    let (mut repository, mut filesystem, task_id) =
        setup_awaiting_design_approval_without_declaration(3);
    repository.high_risk_approvals.insert(
        (task_id, 3, HighRiskCategory::ArchitectureChange),
        HighRiskApprovalRecord {
            task_id,
            approved_task_version: 3,
            risk_category: HighRiskCategory::ArchitectureChange,
            approved_at_ms: 24,
        },
    );
    OperationRiskDeclarationService::new(&mut repository, &mut filesystem)
        .declare_provider_implementation_risk(DeclareProviderImplementationRiskRequest {
            task_id,
            expected_version: 3,
            risk_categories: vec![HighRiskCategory::ArchitectureChange],
            declared_at_ms: 25,
        })
        .expect("declare exact approved category");
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let inputs = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .expect("exact approved assessment starts implementation");

    assert_eq!(inputs.task.state, TaskState::Implementing);
}

#[test]
fn begin_policy_failures_preserve_state_consent_and_provider_boundary() {
    for failure in ["lease", "target", "inspection", "repository", "approval"] {
        let (mut repository, mut filesystem, task_id) = setup_awaiting_design_approval(3);
        match failure {
            "lease" => repository.active_lease = None,
            "target" => {
                filesystem
                    .identities
                    .get_mut(Path::new("C:/managed/task"))
                    .expect("worktree identity")
                    .file_id_hex = "33333333333333333333333333333333".to_owned();
            }
            "inspection" => {
                filesystem.failures.insert(
                    PathBuf::from("C:/managed/task"),
                    FailureCategory::PermissionDenied,
                );
            }
            "repository" => {
                repository.fail_on = Some((
                    "get_operation_risk_declaration",
                    RepositoryErrorCode::DatabaseUnavailable,
                ));
            }
            "approval" => {
                repository
                    .operation_risk_declarations
                    .get_mut(&(task_id, 3, OperationRiskKind::ProviderImplementation))
                    .expect("declaration")
                    .risk_categories = vec![HighRiskCategory::ArchitectureChange];
            }
            _ => unreachable!(),
        }
        let mut time = FakeTime::at(30);
        let mut capability = UnexpectedCapability;

        let error = ImplementationExecutionStarter::new(
            &mut repository,
            &mut time,
            &mut capability,
            &mut filesystem,
        )
        .begin(BeginImplementationExecutionRequest::new(task_id, 3))
        .err()
        .expect("policy failure must reject");

        let expected_code = match failure {
            "lease" => ApplicationErrorCode::ActiveTaskConflict,
            "target" => ApplicationErrorCode::Conflict,
            "inspection" => ApplicationErrorCode::PermissionDenied,
            "repository" => ApplicationErrorCode::StorageUnavailable,
            "approval" => ApplicationErrorCode::InvalidState,
            _ => unreachable!(),
        };
        assert_eq!(error.code(), expected_code, "failure case: {failure}");
        assert_eq!(
            repository.tasks[&task_id].state(),
            TaskState::AwaitingDesignApproval
        );
        assert_eq!(repository.tasks[&task_id].version(), 3);
        assert!(repository.last_saved.is_none());
        assert!(
            repository
                .get_provider_consent(
                    task_id,
                    ProviderKind::Claude,
                    WorkKind::Implementation,
                    3,
                    ContextDataScope::LegacyPhase4,
                )
                .expect("consent lookup")
                .is_none()
        );
    }
}

#[test]
fn begin_rejects_unsupported_capability_with_no_execution_and_state_preserved() {
    let (mut repository, mut filesystem, task_id) = setup_awaiting_design_approval(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Unsupported);

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("unsupported capability must reject before any state change");

    assert_eq!(error.code(), ApplicationErrorCode::Unsupported);
    assert!(
        repository.last_saved.is_none(),
        "no transition may be recorded"
    );
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert_eq!(repository.tasks[&task_id].version(), 3);
}

#[test]
fn begin_rejects_when_task_is_not_awaiting_design_approval() {
    // Evidence (isolation/brief/planning result) is seeded even though the
    // task is already `Implementing`, so this exercises the
    // `TaskService::start_implementation` state check itself rather than
    // being shadowed by an evidence-missing rejection.
    let (mut repository, task_id) = setup_implementing(4);
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 4));
    repository.briefs.insert(task_id, brief_record(task_id));
    repository
        .planning_results
        .insert(task_id, completed_planning_result(task_id, "masked plan"));
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut filesystem = FakeFilesystem::default();

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 4))
    .err()
    .expect("task is already Implementing");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
}

#[test]
fn begin_rejects_a_stale_task_version() {
    let (mut repository, mut filesystem, task_id) = setup_awaiting_design_approval(5);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 1))
    .err()
    .expect("stale expected_version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn begin_rejects_a_duplicate_start_replayed_after_the_first_succeeds() {
    let (mut repository, mut filesystem, task_id) = setup_awaiting_design_approval(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .expect("first start succeeds");

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("a duplicate start replaying the same version must fail closed");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(repository.tasks[&task_id].state(), TaskState::Implementing);
    assert_eq!(repository.tasks[&task_id].version(), 4);
}

#[test]
fn begin_rejects_missing_isolation_record_with_no_state_change() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.briefs.insert(task_id, brief_record(task_id));
    repository
        .planning_results
        .insert(task_id, completed_planning_result(task_id, "masked plan"));
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut filesystem = FakeFilesystem::default();

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("start requires a WorktreeReady isolation record");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert_eq!(repository.tasks[&task_id].version(), 3);
    assert!(repository.last_saved.is_none());
    assert!(
        repository
            .get_provider_consent(
                task_id,
                ProviderKind::Claude,
                WorkKind::Implementation,
                3,
                ContextDataScope::LegacyPhase4
            )
            .expect("consent lookup")
            .is_none(),
        "no consent may be recorded when required evidence is missing"
    );
}

#[test]
fn begin_rejects_missing_planning_result_with_no_state_change() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 3));
    repository.briefs.insert(task_id, brief_record(task_id));
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut filesystem = FakeFilesystem::default();

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("start requires a recorded Claude Planning result");

    assert_eq!(error.code(), ApplicationErrorCode::NotFound);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert!(repository.last_saved.is_none());
}

#[test]
fn begin_rejects_a_non_completed_planning_result_with_no_state_change() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 3));
    repository.briefs.insert(task_id, brief_record(task_id));
    repository.planning_results.insert(
        task_id,
        TaskPlanningResultRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            outcome: PlanningResultOutcome::RecoveryRequired,
            exit_code: None,
            turn_count: None,
            started_at_ms: 5,
            completed_at_ms: 20,
            plan_text: None,
        },
    );
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut filesystem = FakeFilesystem::default();

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("a non-Completed planning result must not start Implementation");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert!(repository.last_saved.is_none());
}

#[test]
fn begin_rejects_missing_brief_with_no_state_change() {
    let (task, history) = restored_task(TaskState::AwaitingDesignApproval, 3, 20, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 3));
    repository
        .planning_results
        .insert(task_id, completed_planning_result(task_id, "masked plan"));
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);
    let mut filesystem = FakeFilesystem::default();

    let error = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .err()
    .expect("start requires a TaskBrief");

    assert_eq!(error.code(), ApplicationErrorCode::Internal);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::AwaitingDesignApproval
    );
    assert!(repository.last_saved.is_none());
}

#[test]
fn run_and_record_success_reaches_testing() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::completed(ImplementationResultOutcome::Completed);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::Testing);
    assert_eq!(executor.observed.len(), 1);
    assert_eq!(executor.observed[0].0, Path::new("C:/managed/task"));
    assert_eq!(executor.observed[0].1, "Add CSV export");
    assert_eq!(executor.observed[0].4, "masked plan");
    assert!(
        repository.active_lease.is_some(),
        "Testing still requires the active lease"
    );
}

#[test]
fn run_and_record_confirmed_cancel_reaches_paused_with_implementing_resume_target() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::completed(ImplementationResultOutcome::Cancelled);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a confirmed cancellation is recorded");

    assert_eq!(view.state, TaskState::Paused);
    assert_eq!(view.resume_target_state, Some(TaskState::Implementing));
    assert!(
        repository.active_lease.is_some(),
        "Paused must keep the active lease"
    );
}

#[test]
fn run_and_record_recovery_required_outcome_keeps_the_lease() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::completed(ImplementationResultOutcome::RecoveryRequired);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("an uncertain outcome is still recorded");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(
        repository.active_lease.is_some(),
        "RecoveryRequired must keep the active lease"
    );
}

#[test]
fn run_and_record_post_transition_preflight_rejection_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::preflight_rejected();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a post-transition preflight rejection is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn run_and_record_executor_failure_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::failing();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a genuine executor failure is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn panic_containment_recovers_from_a_panicking_executor_records_history_and_keeps_the_lease() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a contained executor panic still records RecoveryRequired");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(
        repository.active_lease.is_some(),
        "RecoveryRequired must keep the active lease even after a contained panic"
    );
    let (_, _, record) = repository.last_saved.expect("a transition was recorded");
    assert_eq!(record.from_state(), Some(TaskState::Implementing));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    let stored = repository
        .implementation_results
        .get(&task_id)
        .expect("an implementation result row was recorded for the contained panic");
    assert_eq!(
        stored.outcome,
        ImplementationResultOutcome::RecoveryRequired
    );
}

#[test]
fn panic_containment_never_lets_the_panic_payload_reach_the_recorded_result() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            4,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect("a contained executor panic still records a result");

    let rendered = format!(
        "{view:?} {:?}",
        repository.implementation_results.get(&task_id)
    );
    assert!(
        !rendered.contains(PANIC_SENTINEL),
        "the panic payload must never surface in the recorded TaskView or implementation result"
    );
}

#[test]
fn panic_containment_does_not_report_success_when_the_recovery_write_is_itself_rejected() {
    let (mut repository, task_id) = setup_implementing(4);
    let mut time = FakeTime::at(40);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);
    let stale_expected_version = 99;

    let error = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            stale_expected_version,
            "C:/managed/task",
            &brief,
            "masked plan",
            20,
            &mut executor,
            &cancellation,
        )
        .expect_err("a rejected recovery write must never be reported as success");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::Implementing,
        "the task must be left exactly as it was, not silently advanced to any terminal or recovery state"
    );
    assert!(
        repository.active_lease.is_some(),
        "the lease must remain untouched when the recovery write is rejected"
    );
    assert!(
        repository.last_saved.is_none(),
        "no transition may be recorded when the recovery write is rejected"
    );
}

#[test]
fn begin_then_run_and_record_connects_consent_state_adapter_and_result_end_to_end() {
    let (mut repository, mut filesystem, task_id) = setup_awaiting_design_approval(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let inputs = ImplementationExecutionStarter::new(
        &mut repository,
        &mut time,
        &mut capability,
        &mut filesystem,
    )
    .begin(BeginImplementationExecutionRequest::new(task_id, 3))
    .expect("begin succeeds");
    assert_eq!(inputs.task.state, TaskState::Implementing);

    let consent = repository
        .get_provider_consent(
            task_id,
            ProviderKind::Claude,
            WorkKind::Implementation,
            3,
            ContextDataScope::LegacyPhase4,
        )
        .expect("consent lookup")
        .expect("consent recorded exactly once by begin");
    assert_eq!(consent.approved_task_version, 3);

    let mut executor = ScriptedExecutor::completed(ImplementationResultOutcome::Completed);
    let cancellation = AtomicCancellationSignal::new();

    let view = ImplementationExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            inputs.task.version,
            &inputs.worktree_path,
            &inputs.brief,
            &inputs.plan_text,
            40,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::Testing);
    assert_eq!(executor.observed[0].0, Path::new(&inputs.worktree_path));
    assert_eq!(executor.observed[0].4, "masked plan");
}
