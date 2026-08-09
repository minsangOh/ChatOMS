#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if chatoms_app_lib::run().is_err() {
        eprintln!("ChatOMS failed to start.");
        std::process::exit(1);
    }
}
