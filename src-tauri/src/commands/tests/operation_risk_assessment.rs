use chatoms_domain::{HighRiskCategory, TaskState};
use chatoms_ports::repository::HighRiskApprovalRecord;

use crate::{
    commands::operation_risk_assessment,
    dto::{HighRiskCategoryDto, OperationRiskAssessmentFailureCategoryDto},
};

use super::{operation_risk_support::ready_runtime_for_operation_risk, task_in_state};

#[test]
fn status_is_content_free_and_rejects_stale_or_wrong_state() {
    let task = task_in_state(TaskState::AwaitingDesignApproval);
    let approval = HighRiskApprovalRecord {
        task_id: task.id(),
        approved_task_version: task.version(),
        risk_category: HighRiskCategory::DataMigration,
        approved_at_ms: 1,
    };
    let runtime = ready_runtime_for_operation_risk(task.clone(), vec![approval], false, true);
    let status =
        operation_risk_assessment::handle_get_provider_implementation_risk_assessment_status(
            &runtime,
            &task.id().to_string(),
            task.version(),
        );
    assert_eq!(status.assessment_required, Some(true));
    assert_eq!(status.declaration_exists, Some(false));
    assert!(status.selected_categories.is_empty());
    assert_eq!(status.approval_readiness.len(), 13);
    assert!(status.approval_readiness.iter().any(|entry| {
        entry.risk_category == HighRiskCategoryDto::DataMigration && entry.approved
    }));

    let stale =
        operation_risk_assessment::handle_get_provider_implementation_risk_assessment_status(
            &runtime,
            &task.id().to_string(),
            task.version() + 1,
        );
    assert_failure(
        stale.failure_category,
        OperationRiskAssessmentFailureCategoryDto::VersionConflict,
    );

    let wrong_state = task_in_state(TaskState::Planning);
    let wrong_runtime =
        ready_runtime_for_operation_risk(wrong_state.clone(), Vec::new(), false, true);
    let rejected =
        operation_risk_assessment::handle_get_provider_implementation_risk_assessment_status(
            &wrong_runtime,
            &wrong_state.id().to_string(),
            wrong_state.version(),
        );
    assert_failure(
        rejected.failure_category,
        OperationRiskAssessmentFailureCategoryDto::InvalidState,
    );
}

#[test]
fn declare_requires_explicit_empty_shape_and_approved_categories() {
    let task = task_in_state(TaskState::AwaitingDesignApproval);
    let runtime = ready_runtime_for_operation_risk(task.clone(), Vec::new(), false, true);
    for (categories, explicit_empty) in [
        (Vec::new(), false),
        (vec![HighRiskCategoryDto::DataMigration], true),
        (
            vec![
                HighRiskCategoryDto::DataMigration,
                HighRiskCategoryDto::DataMigration,
            ],
            false,
        ),
    ] {
        let rejected = operation_risk_assessment::handle_declare_provider_implementation_risk(
            &runtime,
            &task.id().to_string(),
            task.version(),
            categories,
            explicit_empty,
        );
        assert_failure(
            rejected.failure_category,
            OperationRiskAssessmentFailureCategoryDto::InvalidInput,
        );
    }

    let unapproved = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &runtime,
        &task.id().to_string(),
        task.version(),
        vec![HighRiskCategoryDto::DataMigration],
        false,
    );
    assert_failure(
        unapproved.failure_category,
        OperationRiskAssessmentFailureCategoryDto::InvalidState,
    );

    let stale = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &runtime,
        &task.id().to_string(),
        task.version() + 1,
        Vec::new(),
        true,
    );
    assert_failure(
        stale.failure_category,
        OperationRiskAssessmentFailureCategoryDto::VersionConflict,
    );

    let wrong_state = task_in_state(TaskState::Planning);
    let wrong_runtime =
        ready_runtime_for_operation_risk(wrong_state.clone(), Vec::new(), false, true);
    let rejected = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &wrong_runtime,
        &wrong_state.id().to_string(),
        wrong_state.version(),
        Vec::new(),
        true,
    );
    assert_failure(
        rejected.failure_category,
        OperationRiskAssessmentFailureCategoryDto::InvalidState,
    );

    let malformed =
        operation_risk_assessment::handle_get_provider_implementation_risk_assessment_status(
            &runtime,
            "not-a-task-id",
            task.version(),
        );
    assert_failure(
        malformed.failure_category,
        OperationRiskAssessmentFailureCategoryDto::InvalidInput,
    );
}

#[test]
fn declare_records_explicit_empty_and_non_empty_immutably() {
    let empty_task = task_in_state(TaskState::AwaitingDesignApproval);
    let empty_runtime =
        ready_runtime_for_operation_risk(empty_task.clone(), Vec::new(), false, true);
    let empty = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &empty_runtime,
        &empty_task.id().to_string(),
        empty_task.version(),
        Vec::new(),
        true,
    );
    assert_eq!(empty.declaration_exists, Some(true));
    assert_eq!(empty.assessment_required, Some(false));
    assert!(empty.selected_categories.is_empty());

    let immutable = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &empty_runtime,
        &empty_task.id().to_string(),
        empty_task.version(),
        Vec::new(),
        true,
    );
    assert_failure(
        immutable.failure_category,
        OperationRiskAssessmentFailureCategoryDto::Internal,
    );

    let selected_task = task_in_state(TaskState::AwaitingDesignApproval);
    let approval = HighRiskApprovalRecord {
        task_id: selected_task.id(),
        approved_task_version: selected_task.version(),
        risk_category: HighRiskCategory::DataMigration,
        approved_at_ms: 1,
    };
    let selected_runtime =
        ready_runtime_for_operation_risk(selected_task.clone(), vec![approval], false, true);
    let selected = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &selected_runtime,
        &selected_task.id().to_string(),
        selected_task.version(),
        vec![HighRiskCategoryDto::DataMigration],
        false,
    );
    assert_eq!(selected.declaration_exists, Some(true));
    assert_eq!(
        selected.selected_categories,
        vec![HighRiskCategoryDto::DataMigration]
    );
}

#[test]
fn declare_maps_identity_and_persistence_failures_safely() {
    let task = task_in_state(TaskState::AwaitingDesignApproval);
    let mismatch_runtime = ready_runtime_for_operation_risk(task.clone(), Vec::new(), false, false);
    let mismatch = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &mismatch_runtime,
        &task.id().to_string(),
        task.version(),
        Vec::new(),
        true,
    );
    assert_failure(
        mismatch.failure_category,
        OperationRiskAssessmentFailureCategoryDto::IdentityMismatch,
    );

    let persistence_runtime =
        ready_runtime_for_operation_risk(task.clone(), Vec::new(), true, true);
    let persistence = operation_risk_assessment::handle_declare_provider_implementation_risk(
        &persistence_runtime,
        &task.id().to_string(),
        task.version(),
        Vec::new(),
        true,
    );
    assert_failure(
        persistence.failure_category,
        OperationRiskAssessmentFailureCategoryDto::PersistenceUnavailable,
    );
}

fn assert_failure(
    actual: Option<OperationRiskAssessmentFailureCategoryDto>,
    expected: OperationRiskAssessmentFailureCategoryDto,
) {
    assert_eq!(actual, Some(expected));
}
