//! The menu-bar item (background mode): status lines, "Clean safe caches now", "Empty Trash…", Open, Quit.

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
    let empty = MenuItem::with_id(app, "empty_trash", "Empty Trash\u{2026}", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Deskveet", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Deskveet", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &next, &PredefinedMenuItem::separator(app)?, &clean, &empty, &open, &PredefinedMenuItem::separator(app)?, &quit])?;
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
            "empty_trash" => empty_trash(app.clone()),
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

/// Empty the Trash from the menu bar: measure it, ask (permanent, so Cancel is the default), then run the
/// Trash rule through the engine. The result arrives as a notification.
fn empty_trash(app: AppHandle) {
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        if st.cleaning.swap(true, Relaxed) {
            return;
        }
        let plat = crate::platform::current();
        use crate::platform::Platform;
        let size = st.engine.trash_size();
        let gb = |b: u64| if b >= 1_000_000_000 { format!("{:.2} GB", b as f64 / 1e9) } else { format!("{:.0} MB", b as f64 / 1e6) };
        if size == 0 {
            st.cleaning.store(false, Relaxed);
            plat.notify("The Trash is empty", "There is nothing to delete.");
            return;
        }
        let ok = plat.confirm(
            "Empty the Trash?",
            &format!("This permanently deletes everything in the Trash ({}). It cannot be undone.", gb(size)),
            "Empty Trash",
        );
        if !ok {
            st.cleaning.store(false, Relaxed);
            return;
        }
        let rep = st.engine.empty_trash(&st.disabled_rules());
        st.cleaning.store(false, Relaxed);
        match rep {
            None => plat.notify("Trash not emptied", "The Trash rule is turned off in Settings, or the Trash is already empty."),
            Some(r) if r.failed == 0 => plat.notify("Trash emptied", &format!("{} freed.", gb(r.freed_bytes))),
            Some(r) => {
                let why = r.results.iter().find_map(|x| x.error.clone()).unwrap_or_else(|| "Some items could not be deleted.".into());
                plat.notify("Trash partly emptied", &format!("{} freed. {why}", gb(r.freed_bytes)));
            }
        }
    });
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
