//! Platform seam. Everything OS-specific lives behind these traits so Windows/Linux can be added
//! by dropping in another module and changing the `Current` alias.

use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod other;

#[cfg(target_os = "macos")]
pub type Current = macos::Mac;
#[cfg(not(target_os = "macos"))]
pub type Current = other::Generic;

/// Hand freed heap pages back to the OS. Cheap; called after refresh bursts to keep the footprint flat.
pub fn trim_memory() {
    #[cfg(target_os = "macos")]
    macos::trim_memory();
}

/// Lower the calling thread's scheduling class so scans never fight the foreground app for CPU/IO.
pub fn background_thread() {
    #[cfg(target_os = "macos")]
    macos::background_thread();
}

/// Where scan caches live (safe to delete at any time).
pub fn cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/app.deskveet.desktop")
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache")).join("deskveet")
    }
}

/// Where durable data (the undo log) lives.
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/app.deskveet.desktop")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("deskveet")
    }
}

pub fn current() -> Current {
    Current::default()
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Pressure {
    #[default]
    Normal,
    Warn,
    Critical,
}

/// Memory numbers the generic sysinfo API cannot provide (all bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct MemExtra {
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
    pub cached: u64,
    pub used: u64,
    pub pressure: Pressure,
    pub swap_used: u64,
    pub swap_total: u64,
}

pub trait Platform: Send + Sync {
    fn memory_extra(&self) -> Option<MemExtra>;
    /// Bytes available *including* purgeable space, if the OS can tell.
    fn available_for_important_usage(&self, mount: &Path) -> Option<u64>;
    /// Is this mount point something the user thinks of as "a disk"?
    fn is_user_volume(&self, mount: &Path, fs: &str) -> bool;
    fn volume_label(&self, mount: &Path, fallback: &str) -> String;
    fn reveal(&self, path: &Path) -> io::Result<()>;
    fn open(&self, path: &Path) -> io::Result<()>;
    fn quick_look(&self, path: &Path) -> io::Result<()>;
    fn open_full_disk_access_settings(&self) -> io::Result<()>;
    /// Does *this running build* have Full Disk Access? `None` when it cannot be told.
    fn full_disk_access(&self) -> Option<bool>;
    /// Show a notification. Falls back to a plain one when the app isn't running from its bundle.
    fn notify(&self, title: &str, body: &str);
    /// Like `notify`, but says why nothing was shown (for example notifications are turned off for the app).
    fn notify_checked(&self, title: &str, body: &str) -> Result<(), String> {
        self.notify(title, body);
        Ok(())
    }
    /// A native yes/no question with the safe choice (Cancel) as the default. `true` only when the person
    /// picked `ok`. Used where there is no window to show our own dialog in, such as the menu bar.
    fn confirm(&self, _title: &str, _message: &str, _ok: &str) -> bool {
        false
    }
    /// Open the system settings page where notifications for this app are turned on or off.
    fn open_notification_settings(&self) -> io::Result<()> {
        Ok(())
    }
    /// Native "save as" and "open" dialogs for a rules file. `None` when cancelled.
    fn pick_save_file(&self, default_name: &str) -> Option<PathBuf>;
    fn pick_file(&self) -> Option<PathBuf>;
    /// Ask the user for a folder with the native chooser. `None` when cancelled.
    fn pick_folder(&self) -> Option<PathBuf>;
}

/// A regular (Dock) application that is running right now.
#[derive(Clone, Debug)]
pub struct RunningApp {
    pub pid: u32,
    pub name: String,
    pub bundle_id: String,
    pub bundle_path: Option<PathBuf>,
}

pub trait Apps: Send + Sync {
    fn running_apps(&self) -> Vec<RunningApp>;
    /// 64px PNG of the app icon.
    fn app_icon_png(&self, bundle_path: &Path) -> Option<Vec<u8>>;
    /// Ask the app to quit (it may ask to save) or kill it. False when the app refused or is gone.
    fn quit_app(&self, pid: u32, force: bool) -> bool;
    /// The process macOS holds responsible for `pid` (an app for its web content, XPC services, ...).
    fn responsible_pid(&self, pid: u32) -> Option<u32>;
}

/// Facts about a process, enough to decide whether we may offer to quit it.
pub struct ProcFacts<'a> {
    pub pid: u32,
    pub uid: u32,
    pub name: &'a str,
    pub exe: Option<&'a Path>,
}

pub trait ProcessInfo: Send + Sync {
    /// `Some(reason)` when the process must never be offered for quit / force quit.
    fn protected_reason(&self, p: &ProcFacts, me_uid: u32, me_pid: u32) -> Option<&'static str>;
    /// The enclosing application bundle (or the executable itself) to reveal in the file manager.
    fn reveal_target(&self, exe: &Path) -> PathBuf;
}
