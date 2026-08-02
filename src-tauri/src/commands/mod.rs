pub mod projects;
pub mod system;
pub mod tasks;

pub const REGISTERED_HANDLERS: [&str; 8] = [
    "get_version",
    "get_health",
    "get_system_status",
    "get_bootstrap_status",
    "list_projects",
    "get_active_task",
    "get_task",
    "list_task_history",
];

#[cfg(test)]
mod tests;
