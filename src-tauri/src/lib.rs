#![doc = "Tauri composition root for the ChatOMS desktop application."]
#![forbid(unsafe_code)]

pub mod bootstrap;
pub mod commands;
pub mod dto;
pub mod error;
pub mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(bootstrap::production_runtime())
        .invoke_handler(tauri::generate_handler![
            commands::system::get_version,
            commands::system::get_health,
            commands::system::get_system_status,
            commands::system::get_bootstrap_status,
            commands::system::get_legacy_migration_diagnostic,
            commands::projects::list_projects,
            commands::projects::inspect_project_candidate,
            commands::projects::register_project,
            commands::projects::get_project_git_status,
            commands::git_isolation::create_isolation_task,
            commands::git_isolation::get_task_isolation,
            commands::git_isolation::approve_git_initialization,
            commands::git_isolation::create_task_worktree,
            commands::tasks::get_active_task,
            commands::tasks::get_task,
            commands::tasks::list_task_history,
            commands::provider_eligibility::get_provider_eligibility,
            commands::provider::set_claude_executable_path,
            commands::provider::refresh_claude_capability,
            commands::planning::start_claude_planning,
            commands::planning::cancel_claude_planning,
            commands::planning::get_planning_result,
            commands::post_merge_validation::get_post_merge_validation_results,
            commands::merge_conflict_inspection::get_merge_conflict_inspection,
            commands::context_package::get_context_package_planning_readiness,
            commands::planning::start_claude_planning_context_package,
            commands::implementation::start_claude_implementation,
            commands::implementation::cancel_claude_implementation,
            commands::context_package::get_context_package_implementation_readiness,
            commands::implementation::start_claude_implementation_context_package,
            commands::testing::start_validation_testing,
            commands::testing::cancel_validation_testing,
            commands::validation_commands::get_validation_command_candidates,
            commands::validation_commands::get_validation_command_approval_status,
            commands::validation_commands::approve_validation_command,
            commands::validation_commands::get_project_root_validation_approval_status,
            commands::validation_commands::approve_project_root_validation,
            commands::review::start_claude_review,
            commands::review::cancel_claude_review,
            commands::review::get_review_result,
            commands::context_package::prepare_planning_context_package,
            commands::context_package::prepare_implementation_context_package,
            commands::context_package::prepare_review_context_package,
            commands::context_package::get_context_package_review_readiness,
            commands::review::start_claude_review_context_package,
            commands::high_risk_approval::get_high_risk_approval_status,
            commands::high_risk_approval::approve_high_risk_operation,
            commands::user_diff_review::get_user_diff_for_review,
            commands::user_diff_review::approve_user_diff,
            commands::merge_execution::approve_user_diff_and_start_merge,
            commands::merge_continue::confirm_manual_resolution_and_start_merge_continue,
            commands::merge_abort::confirm_merge_abort_and_start,
        ])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod artifact_name_tests {
    #[test]
    fn library_and_binary_artifact_basenames_are_distinct() {
        assert_eq!(env!("CARGO_CRATE_NAME"), "chatoms_app_lib");
        assert_ne!(env!("CARGO_CRATE_NAME"), "chatoms_app");
    }
}
