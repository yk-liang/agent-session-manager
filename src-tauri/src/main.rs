// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = agent_session_manager_lib::run_usage_collector_if_requested() {
        std::process::exit(code);
    }
    agent_session_manager_lib::run()
}
