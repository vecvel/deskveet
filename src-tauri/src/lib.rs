mod commands;
pub mod platform;
pub mod sysmon;

use tauri::{AppHandle, Manager, RunEvent, WebviewWindowBuilder, WindowEvent};

/// Show the main window, rebuilding it from tauri.conf.json when it was released to save memory.
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if let Some(cfg) = app.config().app.windows.first() {
        if let Ok(b) = WebviewWindowBuilder::from_config(app, cfg) {
            let _ = b.build();
        }
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .manage(commands::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::processes,
            commands::quit_process,
            commands::reveal,
            commands::open_path,
            commands::quick_look,
            commands::open_fda_settings,
            commands::release_window,
        ])
        // A destroyed window means the WebKit helper processes are gone; drop our caches too.
        .on_window_event(|w, e| {
            if matches!(e, WindowEvent::Destroyed) {
                w.app_handle().state::<commands::AppState>().release_memory();
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build Deskveet");

    app.run(|app, event| match event {
        // Closing the last window keeps the (tiny) process alive; Cmd+Q still quits.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { has_visible_windows, .. } if !has_visible_windows => show_main(app),
        _ => {}
    });
}
