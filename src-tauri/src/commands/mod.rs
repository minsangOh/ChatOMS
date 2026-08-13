pub mod git_isolation;
pub mod planning;
pub mod projects;
pub mod provider;
pub mod provider_eligibility;
pub mod system;
pub mod tasks;

pub const REGISTERED_HANDLERS: [&str; 22] = [
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
];

#[cfg(test)]
mod tests;
