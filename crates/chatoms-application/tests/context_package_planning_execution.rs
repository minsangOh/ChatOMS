mod support;

use std::path::{Path, PathBuf};

use chatoms_application::{
    context_package_planning_execution::{
        BeginContextPackagePlanningExecutionRequest, ContextPackagePlanningExecutionRecorder,
        ContextPackagePlanningExecutionStarter,
    },
    error::ApplicationErrorCode,
};
use chatoms_domain::{ContextDataScope, GitOperationId, ProjectId, TaskId, TaskState, WorkKind};
use chatoms_ports::{
    context_package_planning::ContextPackagePlanningExecutor,
    error::{FailureCategory, PortFailure},
    planning::{PlanningExecutionBrief, PlanningExecutionResult, PlanningExecutionStartOutcome},
    process::{AtomicCancellationSignal, CancellationSignal},
    provider::{
        ProviderCapabilities, ProviderCapabilityPort, ProviderCapabilityStatus, ProviderKind,
    },
    repository::{
        ContextPackageManifestRecord, FoundationRepository, GitIsolationStatus,
        PlanningResultOutcome, ProviderConsent, TaskBriefRecord, TaskGitIsolation,
    },
};

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

type ObservedRun = (PathBuf, String, String, String);

/// Mirrors `planning_execution.rs`'s identically named sentinel: proves the
/// caught panic's payload never surfaces in anything this recorder returns.
const PANIC_SENTINEL: &str = "SIMULATED_EXECUTOR_PANIC_must_never_leak_sk-fake0000000000";

struct ScriptedExecutor {
    scripted: Option<Result<PlanningExecutionStartOutcome, ()>>,
    observed: Vec<ObservedRun>,
    panics: bool,
}

impl ScriptedExecutor {
    fn completed(outcome: PlanningResultOutcome, plan_text: Option<&str>) -> Self {
        Self {
            scripted: Some(Ok(PlanningExecutionStartOutcome::Completed(
                PlanningExecutionResult {
                    outcome,
                    exit_code: Some(0),
                    turn_count: Some(1),
                    plan_text: plan_text.map(str::to_owned),
                },
            ))),
            observed: Vec::new(),
            panics: false,
        }
    }

    fn preflight_rejected() -> Self {
        Self {
            scripted: Some(Ok(PlanningExecutionStartOutcome::PreflightRejected)),
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

    fn panicking() -> Self {
        Self {
            scripted: None,
            observed: Vec::new(),
            panics: true,
        }
    }
}

impl ContextPackagePlanningExecutor for ScriptedExecutor {
    fn start_planning(
        &mut self,
        worktree: &Path,
        brief: PlanningExecutionBrief<'_>,
        _cancellation: &dyn CancellationSignal,
    ) -> Result<PlanningExecutionStartOutcome, PortFailure> {
        self.observed.push((
            worktree.to_path_buf(),
            brief.requirements.to_owned(),
            brief.completion_criteria.to_owned(),
            brief.prohibited_scope.to_owned(),
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

fn seed_context_package_planning_pair(
    repository: &mut FakeRepository,
    task_id: TaskId,
    expected_version: u64,
) {
    let key = (
        task_id,
        ProviderKind::Claude,
        WorkKind::Planning,
        expected_version,
        ContextDataScope::ContextPackageV1,
    );
    repository.consents.insert(
        key,
        ProviderConsent {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            consented_at_ms: 5,
        },
    );
    repository.context_package_manifests.insert(
        key,
        ContextPackageManifestRecord {
            task_id,
            provider: ProviderKind::Claude,
            work_kind: WorkKind::Planning,
            approved_task_version: expected_version,
            data_scope: ContextDataScope::ContextPackageV1,
            created_at_ms: 5,
        },
    );
}

/// A task ready to activate Context Package v1 Planning: `WorktreeReady`
/// state, a matching `WorktreeReady` isolation record, an attached brief,
/// and an already-prepared exact `(Claude, Planning, version,
/// ContextPackageV1)` consent/manifest pair.
fn setup_context_package_ready(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::WorktreeReady, version, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, version));
    repository.briefs.insert(task_id, brief_record(task_id));
    seed_context_package_planning_pair(&mut repository, task_id, version);
    repository.seed_task(task, history);
    (repository, task_id)
}

/// A task already in `Planning`, as `run_and_record` expects to find it.
fn setup_planning(version: u64) -> (FakeRepository, TaskId) {
    let (task, history) = restored_task(TaskState::Planning, version, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository.seed_task(task, history);
    (repository, task_id)
}

#[test]
fn begin_transitions_to_planning_and_returns_worktree_and_brief_when_the_pair_is_prepared() {
    let (mut repository, task_id) = setup_context_package_ready(1);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let inputs =
        ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
            .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 1))
            .expect("begin succeeds");

    assert_eq!(inputs.task.state, TaskState::Planning);
    assert_eq!(inputs.task.version, 2);
    assert_eq!(inputs.worktree_path, "C:/managed/task");
    assert_eq!(inputs.brief.requirements, "Add CSV export");
}

#[test]
fn begin_rejects_when_the_pair_is_not_prepared_with_no_execution_and_state_preserved() {
    let (task, history) = restored_task(TaskState::WorktreeReady, 1, 10, None);
    let task_id = task.id();
    let mut repository = FakeRepository::default();
    repository
        .isolations
        .insert(task_id, worktree_ready_isolation(task_id, 1));
    repository.briefs.insert(task_id, brief_record(task_id));
    repository.seed_task(task, history);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let error =
        ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
            .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 1))
            .expect_err("an unprepared task must reject before any state change");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
    assert!(
        repository.last_saved.is_none(),
        "no transition may be recorded"
    );
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
    assert_eq!(repository.tasks[&task_id].version(), 1);
}

#[test]
fn begin_rejects_unsupported_capability_with_no_execution_and_state_preserved() {
    let (mut repository, task_id) = setup_context_package_ready(1);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Unsupported);

    let error =
        ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
            .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 1))
            .expect_err("unsupported capability must reject before any state change");

    assert_eq!(error.code(), ApplicationErrorCode::Unsupported);
    assert!(repository.last_saved.is_none());
    assert_eq!(repository.tasks[&task_id].state(), TaskState::WorktreeReady);
}

#[test]
fn begin_rejects_when_task_is_not_worktree_ready() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let error =
        ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
            .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 2))
            .expect_err("task is already Planning");

    assert_eq!(error.code(), ApplicationErrorCode::InvalidState);
}

#[test]
fn begin_rejects_a_stale_task_version() {
    let (mut repository, task_id) = setup_context_package_ready(3);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let error =
        ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
            .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 1))
            .expect_err("stale expected_version must be rejected");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
}

#[test]
fn begin_never_creates_or_reuses_a_legacy_phase4_consent() {
    let (mut repository, task_id) = setup_context_package_ready(1);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
        .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 1))
        .expect("begin succeeds");

    let legacy = repository
        .get_provider_consent(
            task_id,
            ProviderKind::Claude,
            WorkKind::Planning,
            1,
            ContextDataScope::LegacyPhase4,
        )
        .expect("consent lookup");
    assert_eq!(
        legacy, None,
        "this path must never create a LegacyPhase4 consent"
    );
}

#[test]
fn run_and_record_success_reaches_awaiting_design_approval() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut executor =
        ScriptedExecutor::completed(PlanningResultOutcome::Completed, Some("masked plan"));
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            2,
            "C:/managed/task",
            &brief,
            5,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::AwaitingDesignApproval);
    assert_eq!(executor.observed.len(), 1);
    assert_eq!(executor.observed[0].0, Path::new("C:/managed/task"));
    assert!(
        repository.active_lease.is_some(),
        "AwaitingDesignApproval still requires the active lease"
    );
}

#[test]
fn run_and_record_post_transition_assembly_or_preflight_rejection_falls_back_to_recovery_required()
{
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    // An assembler rejection (`PayloadTooLarge`/`RedactionFailedClosed`) is
    // folded by `ClaudePlanningAdapter`'s `ContextPackagePlanningExecutor`
    // impl into `PreflightRejected` before this recorder ever sees it (see
    // `chatoms_infrastructure::claude_planning`), so this scripted outcome
    // exercises exactly what this recorder observes for either cause.
    let mut executor = ScriptedExecutor::preflight_rejected();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            2,
            "C:/managed/task",
            &brief,
            5,
            &mut executor,
            &cancellation,
        )
        .expect("a rejection after the transition is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn run_and_record_executor_failure_falls_back_to_recovery_required() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor::failing();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            2,
            "C:/managed/task",
            &brief,
            5,
            &mut executor,
            &cancellation,
        )
        .expect("a genuine executor failure is still recorded, never left stuck");

    assert_eq!(view.state, TaskState::RecoveryRequired);
    assert!(repository.active_lease.is_some());
}

#[test]
fn run_and_record_confirmed_cancel_reaches_cancelled_and_releases_the_lease() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor::completed(PlanningResultOutcome::Cancelled, None);
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            2,
            "C:/managed/task",
            &brief,
            5,
            &mut executor,
            &cancellation,
        )
        .expect("a confirmed cancellation is recorded");

    assert_eq!(view.state, TaskState::Cancelled);
    assert!(
        repository.active_lease.is_none(),
        "a confirmed cancellation must release the active lease"
    );
}

#[test]
fn panic_containment_recovers_from_a_panicking_executor_records_history_and_keeps_the_lease() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            2,
            "C:/managed/task",
            &brief,
            5,
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
    assert_eq!(record.from_state(), Some(TaskState::Planning));
    assert_eq!(record.to_state(), TaskState::RecoveryRequired);
    let stored = repository
        .planning_results
        .get(&task_id)
        .expect("a planning result row was recorded for the contained panic");
    assert_eq!(stored.outcome, PlanningResultOutcome::RecoveryRequired);
    assert_eq!(stored.plan_text, None);
}

#[test]
fn panic_containment_never_lets_the_panic_payload_reach_the_recorded_result() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            2,
            "C:/managed/task",
            &brief,
            5,
            &mut executor,
            &cancellation,
        )
        .expect("a contained executor panic still records a result");

    let rendered = format!("{view:?} {:?}", repository.planning_results.get(&task_id));
    assert!(
        !rendered.contains(PANIC_SENTINEL),
        "the panic payload must never surface in the recorded TaskView or planning result"
    );
}

#[test]
fn panic_containment_does_not_report_success_when_the_recovery_write_is_itself_rejected() {
    let (mut repository, task_id) = setup_planning(2);
    let mut time = FakeTime::at(30);
    let mut executor = ScriptedExecutor::panicking();
    let cancellation = AtomicCancellationSignal::new();
    let brief = brief_record(task_id);
    let stale_expected_version = 99;

    let error = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record_with_panic_containment(
            task_id,
            stale_expected_version,
            "C:/managed/task",
            &brief,
            5,
            &mut executor,
            &cancellation,
        )
        .expect_err("a rejected recovery write must never be reported as success");

    assert_eq!(error.code(), ApplicationErrorCode::VersionConflict);
    assert_eq!(
        repository.tasks[&task_id].state(),
        TaskState::Planning,
        "the task must be left exactly as it was"
    );
    assert!(repository.active_lease.is_some());
    assert!(repository.last_saved.is_none());
}

#[test]
fn begin_then_run_and_record_connects_the_prepared_pair_adapter_and_result_end_to_end() {
    let (mut repository, task_id) = setup_context_package_ready(1);
    let mut time = FakeTime::at(30);
    let mut capability = FakeCapability(ProviderCapabilityStatus::Supported);

    let inputs =
        ContextPackagePlanningExecutionStarter::new(&mut repository, &mut time, &mut capability)
            .begin(BeginContextPackagePlanningExecutionRequest::new(task_id, 1))
            .expect("begin succeeds");
    assert_eq!(inputs.task.state, TaskState::Planning);

    let mut executor =
        ScriptedExecutor::completed(PlanningResultOutcome::Completed, Some("masked plan"));
    let cancellation = AtomicCancellationSignal::new();

    let view = ContextPackagePlanningExecutionRecorder::new(&mut repository, &mut time)
        .run_and_record(
            task_id,
            inputs.task.version,
            &inputs.worktree_path,
            &inputs.brief,
            20,
            &mut executor,
            &cancellation,
        )
        .expect("run and record succeeds");

    assert_eq!(view.state, TaskState::AwaitingDesignApproval);
    assert_eq!(executor.observed[0].0, Path::new(&inputs.worktree_path));
}
