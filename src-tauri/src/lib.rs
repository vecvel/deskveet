pub mod apps;
pub mod cleaner;
pub mod commands;
pub mod fsutil;
pub mod history;
pub mod locate;
pub mod platform;
pub mod rules;
pub mod safety;
pub mod scan;
pub mod schedule;
pub mod sys;
pub mod sysmon;
pub mod tray;
pub mod undo;
pub mod user_rules;
pub mod watch;

use tauri::{AppHandle, Manager, RunEvent, WebviewWindowBuilder, WindowEvent};

/// Show the main window, rebuilding it from tauri.conf.json when it was released to save memory.
pub fn show_main(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
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
        .setup(|app| {
            commands::start_tracker(app.handle().clone());
            commands::start_live(app.handle().clone());
            let on = app.state::<commands::AppState>().ui_get("bg.menubar").and_then(|v| v.as_bool()).unwrap_or(false);
            if on {
                let _ = tray::show(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::processes,
            commands::quit_process,
            commands::reveal,
            commands::open_path,
            commands::quick_look,
            commands::open_fda_settings,
            commands::release_window,
            commands::pick_folder,
            commands::scan_start,
            commands::scan_cancel,
            commands::scan_status,
            commands::scan_load_cached,
            commands::scan_children,
            commands::scan_clear_cache,
            commands::cleanup_state,
            commands::cleanup_scan,
            commands::cleanup_cancel,
            commands::cleanup_plan,
            commands::cleanup_run,
            commands::undo_list,
            commands::undo_restore,
            commands::undo_clear,
            commands::open_trash,
            commands::about,
            commands::fda_status,
            commands::apps_list,
            commands::rules_list,
            commands::locations_info,
            commands::rules_status,
            commands::rules_custom,
            commands::tracking_get,
            commands::alert_set,
            commands::disk_alert_set,
            commands::schedule_set,
            commands::notify_test,
            commands::ui_visible,
            commands::background_get,
            commands::background_set,
            commands::rule_test,
            commands::rule_save,
            commands::rule_delete,
            commands::rules_export,
            commands::rules_import,
            commands::locations_redetect,
            commands::apps_quit,
            commands::ui_get_all,
            commands::ui_set,
            commands::artifact_state,
            commands::artifact_scan,
        ])
        // A destroyed window means the WebKit helper processes are gone; drop our caches too.
        .on_window_event(|w, e| {
            if matches!(e, WindowEvent::Destroyed) {
                let app = w.app_handle();
                let st = app.state::<commands::AppState>();
                st.release_memory();
                // In menu-bar mode the Dock icon goes away with the window.
                #[cfg(target_os = "macos")]
                if st.ui_get("bg.menubar").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
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
