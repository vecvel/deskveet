//! Updates, silently: soon after start and then once a day, ask GitHub whether a newer version is out. The
//! download's signature is checked against the key built into the app before anything is used.
//! macOS: the new app replaces this one on disk at once; the running copy carries on, the new one opens next time.
//! Windows: the installer is kept and runs quietly when Deskveet quits, or the next time it starts.
//! Linux: the AppImage file is replaced at once, like the Mac app.
//! Nothing is shown. Off when the "updates.auto" setting is false, and in development builds.

use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(24 * 3600);
const TIMEOUT: Duration = Duration::from_secs(120);

/// The version already installed (macOS) or downloaded (Windows) in this session, so it is not fetched again.
static DONE: Mutex<Option<String>> = Mutex::new(None);

fn enabled(app: &AppHandle) -> bool {
    !cfg!(debug_assertions) && installed() && app.state::<crate::commands::AppState>().ui_get("updates.auto").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// Windows: only copies the installer put in place update themselves (it leaves its uninstaller beside the
/// program); the portable zip stays as it is. Linux: only the AppImage (its file is replaced), not the package
/// manager's copy. Always true on macOS.
fn installed() -> bool {
    if cfg!(windows) {
        return std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("uninstall.exe").exists())).unwrap_or(false);
    }
    if cfg!(target_os = "linux") {
        return std::env::var_os("APPIMAGE").is_some();
    }
    true
}

/// Start the background checks. Call once at startup.
pub fn start(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    #[cfg(windows)]
    win::install_waiting(&app);
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            if enabled(&app) {
                // a failed check (offline, GitHub down) just waits for the next one
                let _ = tauri::async_runtime::block_on(check(&app));
            }
            std::thread::sleep(EVERY);
        }
    });
}

async fn check(app: &AppHandle) -> Result<(), String> {
    let updater = app.updater_builder().timeout(TIMEOUT).build().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else { return Ok(()) };
    if DONE.lock().unwrap().as_deref() == Some(update.version.as_str()) {
        return Ok(());
    }
    #[cfg(not(windows))]
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    #[cfg(windows)]
    win::keep(&update, update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?)?;
    *DONE.lock().unwrap() = Some(update.version.clone());
    Ok(())
}

/// Windows: run a downloaded installer when Deskveet quits. Call from the exit event; does not return when
/// an update is installed.
pub fn on_exit(app: &AppHandle) {
    #[cfg(windows)]
    win::install_on_exit(app);
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::path::PathBuf;
    use tauri_plugin_updater::Update;

    /// The downloaded installer of the newest version, named after it; at most one is kept.
    fn dir() -> PathBuf {
        crate::platform::cache_dir().join("update")
    }

    fn waiting() -> Option<(String, PathBuf)> {
        std::fs::read_dir(dir()).ok()?.flatten().find_map(|e| {
            let p = e.path();
            let v = p.file_stem()?.to_str()?.to_string();
            (p.extension()? == "exe").then_some((v, p))
        })
    }

    /// Keep a verified installer for later (replacing an older one).
    pub fn keep(update: &Update, bytes: Vec<u8>) -> Result<(), String> {
        let _ = std::fs::remove_dir_all(dir());
        std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
        std::fs::write(dir().join(format!("{}.exe", update.version)), bytes).map_err(|e| e.to_string())
    }

    /// Install the waiting update through the updater, which checks it again and runs the installer quietly.
    /// `restart` opens the new version afterwards. Does not return when the installer starts.
    fn install(app: &AppHandle, restart: bool) -> Result<(), String> {
        let Some((version, path)) = waiting() else { return Ok(()) };
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let updater = app.updater_builder().timeout(Duration::from_secs(8)).restart_after_install(restart).build().map_err(|e| e.to_string())?;
        let update = tauri::async_runtime::block_on(updater.check()).map_err(|e| e.to_string())?;
        match update {
            Some(u) if u.version == version => u.install(bytes).map_err(|e| e.to_string()),
            // a newer one came out (it downloads later), or this one was withdrawn
            _ => {
                let _ = std::fs::remove_dir_all(dir());
                Ok(())
            }
        }
    }

    /// At start: when an installer is waiting from last time, install it before the window is used, and
    /// reopen as the new version.
    pub fn install_waiting(app: &AppHandle) {
        if waiting().is_none() || !enabled(app) {
            return;
        }
        let w = app.get_webview_window("main");
        if let Some(w) = &w {
            let _ = w.hide();
        }
        // returns only when it could not install (offline, say); then carry on with this version
        let _ = install(app, true);
        if let Some(w) = &w {
            let _ = w.show();
        }
    }

    pub fn install_on_exit(app: &AppHandle) {
        if enabled(app) {
            let _ = install(app, false);
        }
    }
}
