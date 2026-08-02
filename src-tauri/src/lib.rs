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
        .manage(bootstrap::production_runtime())
        .invoke_handler(tauri::generate_handler![
            commands::system::get_version,
            commands::system::get_health,
            commands::system::get_system_status,
            commands::system::get_bootstrap_status,
            commands::projects::list_projects,
            commands::tasks::get_active_task,
            commands::tasks::get_task,
            commands::tasks::list_task_history,
        ])
        .run(tauri::generate_context!())
}
