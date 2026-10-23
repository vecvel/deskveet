//! The menu-bar item: status lines, cleaning, apps, keep awake, disks, power, and the window.

use crate::commands::AppState;
use crate::platform::{Apps, Disk, Platform, Power};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

const ID: &str = "deskveet";
const DAY: u64 = 86_400;
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
    if let Some(item) = ITEMS.lock().unwrap().as_ref().map(|i| &i.window) {
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

/// Items whose text, tick or presence changes while the app runs.
struct Items {
    status: MenuItem<Wry>,
    next: MenuItem<Wry>,
    last: MenuItem<Wry>,
    heavy: MenuItem<Wry>,
    awake: Submenu<Wry>,
    awake_modes: [CheckMenuItem<Wry>; 4],
    awake_display: CheckMenuItem<Wry>,
    window: MenuItem<Wry>,
    /// Which optional lines the current menu shows; the menu is rebuilt when this changes.
    shape: Shape,
}

#[derive(Clone, Copy, PartialEq, Default)]
struct Shape {
    last: bool,
    heavy: bool,
    eject: bool,
}

static ITEMS: Mutex<Option<Items>> = Mutex::new(None);
/// The app the "Quit …" line names: pid and bundle id, checked again before quitting.
static HEAVY: Mutex<Option<(u32, String)>> = Mutex::new(None);
/// One long action at a time (dialogs, quitting apps, ejecting).
static BUSY: AtomicBool = AtomicBool::new(false);

/// Keep awake: the running `caffeinate`, its mode, and when it ends.
struct Awake {
    child: Child,
    mode: usize,
    until: Option<u64>,
}
static AWAKE: Mutex<Option<Awake>> = Mutex::new(None);
/// Off, 1 hour, 2 hours, until turned off.
const AWAKE_MODES: [(&str, Option<u64>); 4] = [("Off", None), ("For 1 hour", Some(3600)), ("For 2 hours", Some(7200)), ("Until turned off", None)];

fn build(app: &AppHandle, shape: Shape) -> tauri::Result<(Menu<Wry>, Items)> {
    let item = |id: &str, text: &str, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
    let sep = || PredefinedMenuItem::separator(app);
    let status = item("status", "\u{2026}", false)?;
    let next = item("next", "\u{2026}", false)?;
    let last = item("last", "\u{2026}", false)?;
    let clean = item("clean", "Clean safe caches now", true)?;
    let empty = item("empty_trash", "Empty Trash\u{2026}", true)?;
    let heavy = item("heavy", "\u{2026}", true)?;
    let quit_all = item("quit_all", "Quit all apps\u{2026}", true)?;
    let force_all = item("force_all", "Force quit all apps\u{2026}", true)?;
    let modes = [0, 1, 2, 3].map(|i| CheckMenuItem::with_id(app, format!("awake_{i}"), AWAKE_MODES[i].0, true, i == 0, None::<&str>));
    let [m0, m1, m2, m3] = modes;
    let awake_modes = [m0?, m1?, m2?, m3?];
    let display_on = app.state::<AppState>().ui_get("awake.display").and_then(|v| v.as_bool()).unwrap_or(true);
    let awake_display = CheckMenuItem::with_id(app, "awake_display", "Keep the display on too", true, display_on, None::<&str>)?;
    let awake = Submenu::with_id_and_items(
        app,
        "awake",
        "Keep awake",
        true,
        &[&awake_modes[0], &awake_modes[1], &awake_modes[2], &awake_modes[3], &sep()?, &awake_display],
    )?;
    let eject = item("eject", "Eject all disks\u{2026}", true)?;
    let lock = item("lock", "Lock screen", true)?;
    let sleep = item("sleep", "Sleep", true)?;
    let restart = item("restart", "Restart\u{2026}", true)?;
    let shutdown = item("shutdown", "Shut down\u{2026}", true)?;
    let window = item("open", "Open Deskveet", true)?;
    let quit = item("quit", "Quit Deskveet", true)?;
    let (s1, s2, s3, s4, s5) = (sep()?, sep()?, sep()?, sep()?, sep()?);

    let mut list: Vec<&dyn IsMenuItem<Wry>> = vec![&status, &next];
    if shape.last {
        list.push(&last);
    }
    list.extend([&s1 as &dyn IsMenuItem<Wry>, &clean, &empty, &s2]);
    if shape.heavy {
        list.push(&heavy);
    }
    list.extend([&quit_all as &dyn IsMenuItem<Wry>, &force_all, &s3, &awake]);
    if shape.eject {
        list.push(&eject);
    }
    list.extend([&s4 as &dyn IsMenuItem<Wry>, &lock, &sleep, &restart, &shutdown, &s5, &window, &quit]);
    let menu = Menu::with_items(app, &list)?;
    Ok((menu, Items { status, next, last, heavy, awake, awake_modes, awake_display, window, shape }))
}

pub fn show(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(ID).is_some() {
        return Ok(());
    }
    let (menu, items) = build(app, Shape::default())?;
    TrayIconBuilder::with_id(ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Deskveet")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| on_menu(app, ev.id().as_ref()))
        // Opening the menu brings its lines up to date (in the background; the menu updates while open).
        .on_tray_icon_event(|tray, ev| {
            if matches!(ev, TrayIconEvent::Click { .. }) {
                let app = tray.app_handle().clone();
                std::thread::spawn(move || refresh(&app));
            }
        })
        .build(app)?;
    *ITEMS.lock().unwrap() = Some(items);
    sync(app);
    let app = app.clone();
    std::thread::spawn(move || refresh(&app));
    Ok(())
}

pub fn hide(app: &AppHandle) {
    let _ = app.remove_tray_by_id(ID);
    *ITEMS.lock().unwrap() = None;
    set_awake(app, 0);
}

fn on_menu(app: &AppHandle, id: &str) {
    let app = app.clone();
    match id {
        "open" => toggle_window(&app),
        "quit" => app.exit(0),
        "clean" => clean_now(app),
        "empty_trash" => empty_trash(app),
        "heavy" => quit_heaviest(app),
        "quit_all" => quit_all(app, false),
        "force_all" => quit_all(app, true),
        "eject" => eject_all(app),
        "lock" => {
            let _ = crate::platform::current().lock_screen();
        }
        "sleep" => {
            let _ = crate::platform::current().sleep_now();
        }
        "restart" => power(app, Power::Restart),
        "shutdown" => power(app, Power::ShutDown),
        "awake_display" => {
            let on = ITEMS.lock().unwrap().as_ref().and_then(|i| i.awake_display.is_checked().ok()).unwrap_or(true);
            crate::commands::ui_put(&app.state::<AppState>(), "awake.display", serde_json::Value::Bool(on));
            // restart caffeinate with the same mode and the time that is left
            let current = AWAKE.lock().unwrap().as_ref().map(|a| (a.mode, a.until));
            if let Some((mode, until)) = current {
                start_awake(&app, mode, until.map(|u| u.saturating_sub(crate::scan::now_secs())));
            }
            refresh(&app);
        }
        other => {
            if let Some(mode) = other.strip_prefix("awake_").and_then(|m| m.parse::<usize>().ok()).filter(|m| *m < 4) {
                set_awake(&app, mode);
                refresh(&app);
            }
        }
    }
}

// ── status lines ──

/// Bring every line of the menu up to date, rebuilding the menu when an optional line appears or goes.
/// Called every minute, when the menu is opened, and after actions.
pub fn refresh(app: &AppHandle) {
    let st = app.state::<AppState>();
    let now = crate::scan::now_secs();
    let plat = crate::platform::current();

    let free = crate::fsutil::free_bytes(st.engine.home());
    let pressure = plat.memory_extra().map(|m| format!("{:?}", m.pressure)).unwrap_or_default();
    let status = format!("{} free  \u{00b7}  Memory: {}", bytes_h(free), pressure);
    let next = next_clean_text(&st, now);
    let last = last_clean_text(&st, now);
    let heavy = heaviest_app(&st);
    *HEAVY.lock().unwrap() = heavy.as_ref().map(|(pid, id, _)| (*pid, id.clone()));
    let has_disks = !plat.ejectable_disks().is_empty();
    // keep awake: forget a caffeinate that ended by itself (its time ran out)
    let awake = {
        let mut a = AWAKE.lock().unwrap();
        if a.as_mut().is_some_and(|x| x.child.try_wait().ok().flatten().is_some()) {
            *a = None;
        }
        a.as_ref().map(|x| (x.mode, x.until))
    };

    let shape = Shape { last: last.is_some(), heavy: heavy.is_some(), eject: has_disks };
    let rebuild = ITEMS.lock().unwrap().as_ref().is_some_and(|i| i.shape != shape);
    if rebuild {
        if let (Some(tray), Ok((menu, items))) = (app.tray_by_id(ID), build(app, shape)) {
            let _ = tray.set_menu(Some(menu));
            *ITEMS.lock().unwrap() = Some(items);
            sync(app);
        }
    }
    let guard = ITEMS.lock().unwrap();
    let Some(i) = guard.as_ref() else { return };
    let _ = i.status.set_text(status);
    let _ = i.next.set_text(next);
    if let Some(l) = last {
        let _ = i.last.set_text(l);
    }
    if let Some((_, _, label)) = heavy {
        let _ = i.heavy.set_text(label);
    }
    let mode = awake.map_or(0, |(m, _)| m);
    for (k, item) in i.awake_modes.iter().enumerate() {
        let _ = item.set_checked(k == mode);
    }
    let _ = i.awake.set_text(match awake {
        None => "Keep awake".to_string(),
        Some((_, Some(until))) => format!("Keep awake  \u{00b7}  {} left", minutes_h(until.saturating_sub(now))),
        Some((_, None)) => "Keep awake  \u{00b7}  on".to_string(),
    });
}

fn bytes_h(b: u64) -> String {
    if b >= 1_000_000_000 { format!("{:.1} GB", b as f64 / 1e9) } else { format!("{:.0} MB", b as f64 / 1e6) }
}

fn minutes_h(secs: u64) -> String {
    let m = secs.div_ceil(60);
    if m >= 60 { format!("{} h {} min", m / 60, m % 60) } else { format!("{m} min") }
}

fn next_clean_text(st: &AppState, now: u64) -> String {
    let cfg = st.tracking.cfg.lock().unwrap().clone();
    let reg = st.engine.registry();
    let soonest = cfg
        .schedules
        .iter()
        .map(|(id, s)| {
            let next = crate::schedule::last_due(s, now) + if s.every == crate::schedule::Every::Daily { DAY } else { 7 * DAY };
            (next, reg.get(id).map(|r| r.name.clone()).unwrap_or_default())
        })
        .min_by_key(|(t, _)| *t);
    match soonest {
        Some((t, name)) => {
            let l = crate::schedule::local(t);
            format!("Next clean: {name}, {:02}/{:02} {:02}:00", l.day, l.month, l.hour)
        }
        None => "No scheduled cleaning".into(),
    }
}

/// "Last clean: 2.4 GB, 3 h ago" from the undo log: the newest run that did something.
fn last_clean_text(st: &AppState, now: u64) -> Option<String> {
    let entries = st.engine.undo.list();
    let done = |e: &&crate::undo::Entry| e.status == "ok" && e.action != "dry-run";
    let newest = entries.iter().filter(done).max_by_key(|e| e.ts)?;
    let bytes: u64 = entries.iter().filter(done).filter(|e| e.run == newest.run).map(|e| e.bytes).sum();
    let ago = now.saturating_sub(newest.ts);
    let when = match ago {
        0..=119 => "just now".to_string(),
        120..=3599 => format!("{} min ago", ago / 60),
        3600..=86_399 => format!("{} h ago", ago / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{} days ago", ago / DAY),
    };
    Some(format!("Last clean: {}, {when}", bytes_h(bytes)))
}

/// The regular app using the most memory, as (pid, bundle id, menu label).
fn heaviest_app(st: &AppState) -> Option<(u32, String, String)> {
    let me = std::process::id();
    let mut mon = st.mon.lock().unwrap();
    let apps: Vec<_> = mon.plat().running_apps().into_iter().filter(|a| crate::apps::protected_reason(a, me).is_none()).collect();
    let usage = mon.app_usage(&apps);
    let (app, (mem, _, _)) = apps.iter().zip(usage).max_by_key(|(_, (m, _, _))| *m)?;
    Some((app.pid, app.bundle_id.clone(), format!("Quit {} ({})", app.name, bytes_h(mem))))
}

// ── apps ──

/// Regular apps that may be quit (never Finder, the Dock or Deskveet), with unique display names.
fn quittable_apps() -> Vec<(String, crate::platform::RunningApp)> {
    let me = std::process::id();
    let mut apps: Vec<_> = crate::platform::current().running_apps().into_iter().filter(|a| crate::apps::protected_reason(a, me).is_none()).collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    let mut seen = std::collections::HashMap::<String, usize>::new();
    apps.into_iter()
        .map(|a| {
            let n = seen.entry(a.name.clone()).or_default();
            *n += 1;
            let label = if *n == 1 { a.name.clone() } else { format!("{} ({})", a.name, n) };
            (label, a)
        })
        .collect()
}

fn quit_heaviest(app: AppHandle) {
    std::thread::spawn(move || {
        let target = HEAVY.lock().unwrap().clone();
        let plat = crate::platform::current();
        if let Some((pid, id)) = target {
            if let Some(a) = plat.running_apps().into_iter().find(|a| a.pid == pid && a.bundle_id == id) {
                if !plat.quit_app(a.pid, false) {
                    plat.notify(&format!("{} did not quit", a.name), "It may be waiting for you to save your work.");
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        refresh(&app);
    });
}

/// Quit (or force quit) every regular app, after a list where any can be selected to stay open.
fn quit_all(app: AppHandle, force: bool) {
    if BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let plat = crate::platform::current();
        let apps = quittable_apps();
        let done = || BUSY.store(false, Relaxed);
        if apps.is_empty() {
            plat.notify("No apps to quit", "Only Finder and Deskveet are open.");
            return done();
        }
        let names: Vec<String> = apps.iter().map(|(n, _)| n.clone()).collect();
        let (title, prompt, ok) = if force {
            ("Force quit all apps", "Select any apps to keep open (\u{2318}-click to pick several). All the others close at once and lose unsaved work.", "Force Quit Apps")
        } else {
            ("Quit all apps", "Select any apps to keep open (\u{2318}-click to pick several). All the others are asked to quit, and can still save their work.", "Quit Apps")
        };
        let Some(keep) = plat.choose_from_list(title, prompt, &names, ok) else { return done() };
        let targets: Vec<_> = apps.into_iter().filter(|(n, _)| !keep.contains(n)).collect();
        if targets.is_empty() {
            return done();
        }
        if force {
            let list = targets.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
            let n = targets.len();
            if !plat.confirm(&format!("Force quit {n} app{}?", if n == 1 { "" } else { "s" }), &format!("Unsaved work in these is lost: {list}."), "Force Quit") {
                return done();
            }
        }
        let failed: Vec<String> = targets.iter().filter(|(_, a)| !plat.quit_app(a.pid, force)).map(|(n, _)| n.clone()).collect();
        let n = targets.len() - failed.len();
        let verb = if force { "Force quit" } else { "Asked" };
        let what = if force { format!("{n} app{}.", if n == 1 { "" } else { "s" }) } else { format!("{n} app{} to quit.", if n == 1 { "" } else { "s" }) };
        let tail = if failed.is_empty() { String::new() } else { format!(" Did not quit: {}.", failed.join(", ")) };
        plat.notify(if force { "Apps force quit" } else { "Apps quitting" }, &format!("{verb} {what}{tail}"));
        done();
        std::thread::sleep(std::time::Duration::from_secs(2));
        refresh(&app);
    });
}

// ── keep awake ──

fn start_awake(app: &AppHandle, mode: usize, secs: Option<u64>) {
    let display = app.state::<AppState>().ui_get("awake.display").and_then(|v| v.as_bool()).unwrap_or(true);
    let mut a = AWAKE.lock().unwrap();
    if let Some(mut old) = a.take() {
        let _ = old.child.kill();
        let _ = old.child.wait();
    }
    if mode == 0 {
        return;
    }
    if let Ok(child) = crate::platform::current().keep_awake(display, secs) {
        *a = Some(Awake { child, mode, until: secs.map(|s| crate::scan::now_secs() + s) });
    }
}

fn set_awake(app: &AppHandle, mode: usize) {
    start_awake(app, mode, AWAKE_MODES[mode].1);
}

// ── disks and power ──

fn eject_all(app: AppHandle) {
    if BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let plat = crate::platform::current();
        let disks: Vec<Disk> = plat.ejectable_disks();
        if disks.is_empty() {
            plat.notify("No disks to eject", "There are no external drives or disk images mounted.");
        } else {
            let list = disks.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ");
            if plat.confirm("Eject all disks?", &format!("Ejects {list}. Save and close any files open on them first."), "Eject") {
                let errors: Vec<String> = disks.iter().filter_map(|d| plat.eject(d).err()).collect();
                if errors.is_empty() {
                    plat.notify("Disks ejected", &format!("{list} can be unplugged."));
                } else {
                    plat.notify("Some disks were not ejected", &errors.join(" "));
                }
            }
        }
        BUSY.store(false, Relaxed);
        refresh(&app);
    });
}

fn power(app: AppHandle, what: Power) {
    if BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let plat = crate::platform::current();
        let (title, ok) = match what {
            Power::Restart => ("Restart your Mac?", "Restart"),
            Power::ShutDown => ("Shut down your Mac?", "Shut Down"),
        };
        if plat.confirm(title, "Open apps are asked to quit first, and can still save their work.", ok) {
            if let Err(e) = plat.power(what) {
                plat.notify(&format!("Could not {}", ok.to_lowercase()), &e);
            }
        }
        BUSY.store(false, Relaxed);
        let _ = app;
    });
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
