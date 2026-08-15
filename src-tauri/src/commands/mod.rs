pub mod context_package;
pub mod git_isolation;
pub mod high_risk_approval;
pub mod implementation;
pub mod planning;
pub mod projects;
pub mod provider;
pub mod provider_eligibility;
pub mod review;
pub mod system;
pub mod tasks;
pub mod testing;
pub mod user_diff_review;
pub mod validation_commands;

pub const REGISTERED_HANDLERS: [&str; 45] = [
    "get_version",
    "get_health",
    "get_system_status",
    "get_bootstrap_status",
    "get_legacy_migration_diagnostic",
    "list_projects",
    "inspect_project_candidate",
    "register_project",
    "get_project_git_status",
    "create_isolation_task",
    "get_task_isolation",
    "approve_git_initialization",
    "create_task_worktree",
    "get_active_task",
    "get_task",
    "list_task_history",
    "get_provider_eligibility",
    "set_claude_executable_path",
    "refresh_claude_capability",
    "start_claude_planning",
    "cancel_claude_planning",
    "get_planning_result",
    "get_context_package_planning_readiness",
    "start_claude_planning_context_package",
    "start_claude_implementation",
    "cancel_claude_implementation",
    "get_context_package_implementation_readiness",
    "start_claude_implementation_context_package",
    "start_validation_testing",
    "cancel_validation_testing",
    "get_validation_command_candidates",
    "get_validation_command_approval_status",
    "approve_validation_command",
    "start_claude_review",
    "cancel_claude_review",
    "get_review_result",
    "prepare_planning_context_package",
    "prepare_implementation_context_package",
    "prepare_review_context_package",
    "get_context_package_review_readiness",
    "start_claude_review_context_package",
    "get_high_risk_approval_status",
    "approve_high_risk_operation",
    "get_user_diff_for_review",
    "approve_user_diff",
];

#[cfg(test)]
mod tests;
