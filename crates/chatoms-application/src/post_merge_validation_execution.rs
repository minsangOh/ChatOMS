use chatoms_domain::ValidationCommandKind;
use chatoms_ports::{
    TimeProvider,
    error::FailureCategory,
    process::CancellationSignal,
    repository::PostMergeValidationResultOutcome,
    validation_execution::{
        ValidationCommandExecutor, ValidationExecutionOutcome, ValidationExecutionRequest,
        ValidationExecutionStartOutcome,
    },
};

use crate::{
    error::ApplicationError,
    post_merge_validation::PostMergeValidationInputs,
    tasks::{
        AppendPostMergeValidationResultRequest, FinalizePostMergeValidationBatchRequest,
        TaskService, TaskView,
    },
};

const ACTOR_KIND: &str = "application";
const RESULT_REASON: &str = "task.post-merge-validation.result";

pub struct PostMergeValidationRecorder<'a, R, T> {
    repository: &'a mut R,
    time: &'a mut T,
}

impl<'a, R, T> PostMergeValidationRecorder<'a, R, T>
where
    R: chatoms_ports::repository::FoundationRepository,
    T: TimeProvider,
{
    #[must_use]
    pub const fn new(repository: &'a mut R, time: &'a mut T) -> Self {
        Self { repository, time }
    }

    pub fn run_and_record_with_panic_containment<X>(
        &mut self,
        inputs: &PostMergeValidationInputs,
        executor: &mut X,
        cancellation: &dyn CancellationSignal,
    ) -> Result<TaskView, ApplicationError>
    where
        X: ValidationCommandExecutor,
    {
        if inputs.approvals.len() != 2
            || inputs.approvals[0].kind != ValidationCommandKind::Test
            || inputs.approvals[1].kind != ValidationCommandKind::Build
        {
            return Err(category_error(FailureCategory::InvariantViolation));
        }

        for (index, approval) in inputs.approvals.iter().enumerate() {
            let started_at_ms = self.now_ms()?;
            let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                executor.start_validation_command(
                    ValidationExecutionRequest {
                        target: &inputs.target,
                        approval,
                    },
                    cancellation,
                )
            }));
            let (outcome, exit_code) = match attempt {
                Ok(Ok(ValidationExecutionStartOutcome::Completed(outcome))) => classify(outcome),
                Ok(Ok(ValidationExecutionStartOutcome::BindingRejected(_))) => {
                    (PostMergeValidationResultOutcome::BindingRejected, None)
                }
                Ok(Err(_)) | Err(_) => (PostMergeValidationResultOutcome::Uncertain, None),
            };
            if outcome == PostMergeValidationResultOutcome::Success
                && index == 0
                && approval.kind == ValidationCommandKind::Test
            {
                let completed_at_ms = self.now_ms()?;
                TaskService::new(self.repository, self.time).append_post_merge_validation_result(
                    AppendPostMergeValidationResultRequest::new(
                        inputs.task.id,
                        inputs.approval_task_version,
                        inputs.task.version,
                        approval.kind,
                        exit_code,
                        safe_summary(outcome),
                        started_at_ms,
                        completed_at_ms,
                    ),
                )?;
                continue;
            }

            return TaskService::new(self.repository, self.time)
                .finalize_post_merge_validation_batch(
                    FinalizePostMergeValidationBatchRequest::new(
                        inputs.task.id,
                        inputs.approval_task_version,
                        inputs.task.version,
                        approval.kind,
                        outcome,
                        exit_code,
                        safe_summary(outcome),
                        started_at_ms,
                        ACTOR_KIND.to_owned(),
                        RESULT_REASON.to_owned(),
                    ),
                );
        }

        Err(category_error(FailureCategory::Internal))
    }

    fn now_ms(&mut self) -> Result<i64, ApplicationError> {
        self.time
            .now_ms()
            .map_err(|error| ApplicationError::from_categorized(&error))
    }
}

fn classify(
    outcome: ValidationExecutionOutcome,
) -> (PostMergeValidationResultOutcome, Option<i32>) {
    match outcome {
        ValidationExecutionOutcome::Success => (PostMergeValidationResultOutcome::Success, Some(0)),
        ValidationExecutionOutcome::ExitFailure { exit_code } => (
            PostMergeValidationResultOutcome::ExitFailure,
            Some(exit_code),
        ),
        ValidationExecutionOutcome::TimedOut => (PostMergeValidationResultOutcome::TimedOut, None),
        ValidationExecutionOutcome::StdoutBoundExceeded => {
            (PostMergeValidationResultOutcome::StdoutBoundExceeded, None)
        }
        ValidationExecutionOutcome::Cancelled => {
            (PostMergeValidationResultOutcome::Cancelled, None)
        }
        ValidationExecutionOutcome::Uncertain => {
            (PostMergeValidationResultOutcome::Uncertain, None)
        }
    }
}

fn safe_summary(outcome: PostMergeValidationResultOutcome) -> String {
    match outcome {
        PostMergeValidationResultOutcome::Success => "post-merge validation completed successfully",
        PostMergeValidationResultOutcome::ExitFailure => {
            "post-merge validation exited with a nonzero status"
        }
        PostMergeValidationResultOutcome::TimedOut => {
            "post-merge validation exceeded its time limit"
        }
        PostMergeValidationResultOutcome::StdoutBoundExceeded => {
            "post-merge validation output exceeded the allowed size"
        }
        PostMergeValidationResultOutcome::BindingRejected => {
            "post-merge validation binding was rejected"
        }
        PostMergeValidationResultOutcome::Cancelled => "post-merge validation was cancelled",
        PostMergeValidationResultOutcome::Uncertain => {
            "post-merge validation outcome could not be confirmed"
        }
    }
    .to_owned()
}

fn category_error(category: FailureCategory) -> ApplicationError {
    ApplicationError::from_failure(
        category,
        category.default_severity(),
        category.default_retry(),
    )
}
