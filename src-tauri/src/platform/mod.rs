//! Platform seam. Everything OS-specific lives behind these traits so Windows/Linux can be added
//! by dropping in another module and changing the `Current` alias.

use serde::Serialize;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod other;
#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "macos")]
pub type Current = macos::Mac;
#[cfg(windows)]
pub type Current = windows::Win;
#[cfg(target_os = "linux")]
pub type Current = linux::Linux;
#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
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

/// The user's home folder (`HOME` on macOS and Linux, the profile folder on Windows).
pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// A per-user Windows folder from the environment (`APPDATA`, `LOCALAPPDATA`), or its usual place.
#[cfg(windows)]
fn win_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home_dir().join(fallback))
}

/// Where scan caches live (safe to delete at any time).
pub fn cache_dir() -> PathBuf {
    #[cfg(windows)]
    return win_dir("LOCALAPPDATA", "AppData/Local").join("app.deskveet.desktop").join("cache");
    #[cfg(not(windows))]
    let home = home_dir();
    #[cfg(not(windows))]
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/app.deskveet.desktop")
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache")).join("app.deskveet.desktop")
    }
}

/// Where durable data (the undo log) lives.
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    return win_dir("APPDATA", "AppData/Roaming").join("app.deskveet.desktop");
    #[cfg(not(windows))]
    let home = home_dir();
    #[cfg(not(windows))]
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/app.deskveet.desktop")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("app.deskveet.desktop")
    }
}

/// The user's own Trash folder: `~/.Trash` on macOS; on Windows the user's folder in the system drive's
/// Recycle Bin (`C:\\$Recycle.Bin\\<user SID>`), which holds the deleted items as `$R...` files next to
/// `$I...` files that record where each came from.
pub fn trash_dir(home: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = home; // the Recycle Bin is on the system drive, not in the profile
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        return match windows_user_sid() {
            Some(sid) => PathBuf::from(format!("{drive}\\$Recycle.Bin\\{sid}")),
            None => PathBuf::from(format!("{drive}\\$Recycle.Bin")),
        };
    }
    #[cfg(target_os = "macos")]
    return home.join(".Trash");
    // the freedesktop Trash: `files/` holds the items, `info/` one `.trashinfo` record per item
    #[cfg(not(any(windows, target_os = "macos")))]
    return std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("Trash/files");
}

/// The SID of the user this app runs as, like `S-1-5-21-...-1001`.
#[cfg(windows)]
pub fn windows_user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: standard token query; every buffer is sized as reported and every handle is released.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) != 0;
        CloseHandle(token);
        if !ok {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut wide: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
            return None;
        }
        let n = (0..).take_while(|&i| *wide.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, n));
        LocalFree(wide.cast());
        Some(sid)
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
    /// Ask the person to pick any number of `items` (none is fine). `None` when cancelled.
    fn choose_from_list(&self, _title: &str, _prompt: &str, _items: &[String], _ok: &str) -> Option<Vec<String>> {
        None
    }
    /// Apps installed where people install them (Applications folders), with their ids and versions.
    fn installed_apps(&self) -> Vec<Bundle> {
        Vec::new()
    }
    /// The bundle ids of every app the system knows about anywhere (including helper apps inside
    /// other apps). Used to tell whether an app's leftover data still has an app.
    fn known_bundle_ids(&self) -> HashSet<String> {
        HashSet::new()
    }
    /// Launch agents and daemons defined in the standard folders.
    fn launch_defs(&self) -> Vec<LaunchDef> {
        Vec::new()
    }
    /// Open the system page where login items and background items are turned on or off.
    fn open_login_items_settings(&self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// When each file was last opened, as far as the system knows (unix seconds); same order as `paths`.
    fn last_used(&self, paths: &[PathBuf]) -> Vec<Option<i64>> {
        vec![None; paths.len()]
    }
    /// Bytes of a file that no other file shares (APFS clones share theirs); what removing it would free.
    fn private_size(&self, _path: &Path) -> Option<u64> {
        None
    }
    /// Put the computer to sleep now.
    fn sleep_now(&self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// Lock the screen now.
    fn lock_screen(&self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    /// Restart or shut down the way the system menu does (apps are asked to quit and can save).
    fn power(&self, _what: Power) -> Result<(), String> {
        Err("Not supported on this system".into())
    }
    /// Disks that can be ejected right now; never the start-up disk.
    fn ejectable_disks(&self) -> Vec<Disk> {
        Vec::new()
    }
    fn eject(&self, _disk: &Disk) -> Result<(), String> {
        Err("Not supported on this system".into())
    }
    /// Keep the computer (and optionally the display) awake until the returned process ends: after
    /// `secs` when given, or when it is killed. It also ends when this app exits.
    fn keep_awake(&self, _display: bool, _secs: Option<u64>) -> io::Result<std::process::Child> {
        Err(io::ErrorKind::Unsupported.into())
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
/// A disk that can be ejected (external drive or mounted disk image).
#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub name: String,
    pub path: PathBuf,
}

/// An app bundle on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    pub path: PathBuf,
    pub bundle_id: String,
    pub version: String,
}

/// A launch agent or daemon definition (a property list in a LaunchAgents or LaunchDaemons folder).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchDef {
    pub plist: PathBuf,
    pub label: String,
    /// The program it starts (first argument), if it names one.
    pub program: Option<String>,
    pub run_at_load: bool,
    pub keep_alive: bool,
    pub disabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Power {
    Restart,
    ShutDown,
}

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
