//! The menu-bar item (background mode): status lines, "Clean safe caches now", Open, Quit.

use crate::commands::AppState;
use std::sync::atomic::Ordering::Relaxed;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

const ID: &str = "deskveet";

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
            "open" => crate::show_main(app),
            "quit" => app.exit(0),
            "clean" => clean_now(app.clone()),
            _ => {}
        })
        .build(app)?;
    let st = app.state::<AppState>();
    *st.tray_lines.lock().unwrap() = Some((status, next));
    crate::commands::refresh_tray_lines(&st, crate::scan::now_secs());
    Ok(())
}

pub fn hide(app: &AppHandle) {
    let _ = app.remove_tray_by_id(ID);
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
