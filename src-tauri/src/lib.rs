mod commands;
pub mod platform;
pub mod sysmon;

pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::processes,
            commands::quit_process,
            commands::reveal,
            commands::open_path,
            commands::quick_look,
            commands::open_fda_settings,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Deskveet");
}
