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
