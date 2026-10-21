//! The menu-bar item (background mode): status lines, "Clean safe caches now", Open, Quit.

use crate::commands::AppState;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

const ID: &str = "deskveet";

/// The Open/Show/Hide item, kept so its label can follow the window.
static WINDOW_ITEM: Mutex<Option<MenuItem<tauri::Wry>>> = Mutex::new(None);
/// Whether the main window has keyboard focus (from window events; there is no getter for it).
static FOCUSED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
enum WindowState { Hidden, Behind, Front }

fn window_state(app: &AppHandle) -> WindowState {
    match app.get_webview_window("main") {
        Some(w) if w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) =>
            if FOCUSED.load(Relaxed) { WindowState::Front } else { WindowState::Behind },
        _ => WindowState::Hidden,
    }
}

/// Called on window focus, resize (minimise) and destroy events.
pub fn window_changed(app: &AppHandle, focused: Option<bool>) {
    if let Some(f) = focused {
        FOCUSED.store(f, Relaxed);
    }
    sync(app);
}

fn sync(app: &AppHandle) {
    if let Some(item) = WINDOW_ITEM.lock().unwrap().as_ref() {
        let _ = item.set_text(match window_state(app) {
            WindowState::Hidden => "Open Deskveet",
            WindowState::Behind => "Show Deskveet",
            WindowState::Front => "Hide Deskveet",
        });
    }
}

fn toggle_window(app: &AppHandle) {
    if window_state(app) == WindowState::Front {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        FOCUSED.store(false, Relaxed);
    } else {
        crate::show_main(app);
    }
    sync(app);
}

pub fn show(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(ID).is_some() {
        return Ok(());
    }
    let status = MenuItem::with_id(app, "status", "\u{2026}", false, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "\u{2026}", false, None::<&str>)?;
    let clean = MenuItem::with_id(app, "clean", "Clean safe caches now", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Deskveet", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Deskveet", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &next, &PredefinedMenuItem::separator(app)?, &clean, &open, &PredefinedMenuItem::separator(app)?, &quit])?;
    TrayIconBuilder::with_id(ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Deskveet")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => toggle_window(app),
            "quit" => app.exit(0),
            "clean" => clean_now(app.clone()),
            _ => {}
        })
        .build(app)?;
    *WINDOW_ITEM.lock().unwrap() = Some(open);
    sync(app);
    let st = app.state::<AppState>();
    *st.tray_lines.lock().unwrap() = Some((status, next));
    crate::commands::refresh_tray_lines(&st, crate::scan::now_secs());
    Ok(())
}

pub fn hide(app: &AppHandle) {
    let _ = app.remove_tray_by_id(ID);
    *WINDOW_ITEM.lock().unwrap() = None;
    *app.state::<AppState>().tray_lines.lock().unwrap() = None;
}

fn clean_now(app: AppHandle) {
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        if st.cleaning.swap(true, Relaxed) {
            return;
        }
        let rep = st.engine.clean_safe_now(&st.disabled_rules());
        st.cleaning.store(false, Relaxed);
        let plat = crate::platform::current();
        use crate::platform::Platform;
        let gb = rep.trashed_bytes as f64 / 1e9;
        plat.notify(
            "Safe caches cleaned",
            &if rep.ok > 0 {
                format!("Moved {} items ({gb:.2} GB) to the Trash.{}", rep.ok, if rep.skipped > 0 { format!(" {} in use were left alone.", rep.skipped) } else { String::new() })
            } else {
                "Nothing to clean right now.".into()
            },
        );
    });
}
