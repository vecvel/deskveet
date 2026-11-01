//! Linux: memory, drives, the file manager, notifications, dialogs, power and running apps. Everything goes
//! through the desktop's own services (D-Bus, systemd, udisks) and the tools every desktop ships.

use super::{Apps, Disk, MemExtra, Platform, Power, Pressure, ProcFacts, ProcessInfo, RunningApp};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Default)]
pub struct Linux;

/// Whether a program is on `PATH`.
pub fn has(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

/// Start a program and forget it (reaped on a thread so no zombie is left).
fn spawn(cmd: &mut Command) -> io::Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Run a program and return its trimmed output when it succeeds.
fn output(cmd: &mut Command) -> Option<String> {
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// A string as GVariant text, for `gdbus call` arguments.
fn gv(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn file_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// `/proc/meminfo` in bytes.
fn meminfo() -> std::collections::HashMap<String, u64> {
    std::fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let kb: u64 = v.split_whitespace().next()?.parse().ok()?;
            Some((k.to_string(), kb * 1024))
        })
        .collect()
}

/// The `avg10` share of time some task waited for memory (`/proc/pressure/memory`), when the kernel reports it.
fn memory_pressure() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/pressure/memory").ok()?;
    let some = s.lines().find(|l| l.starts_with("some"))?;
    some.split_whitespace().find_map(|f| f.strip_prefix("avg10=")).and_then(|v| v.parse().ok())
}

/// Mounted filesystems from `/proc/mounts`: (device, mount point, type).
pub fn mounts() -> Vec<(String, PathBuf, String)> {
    // mount points escape spaces and a few other characters as octal
    let unescape = |s: &str| {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c == '\\' {
                let oct: String = (0..3).filter_map(|_| it.next()).collect();
                if let Ok(n) = u8::from_str_radix(&oct, 8) {
                    out.push(n as char);
                    continue;
                }
                out.push('\\');
                out.push_str(&oct);
            } else {
                out.push(c);
            }
        }
        out
    };
    std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((f.next()?.to_string(), PathBuf::from(unescape(f.next()?)), f.next()?.to_string()))
        })
        .collect()
}

/// Where removable drives are mounted: `/media/<user>/<drive>`, `/run/media/<user>/<drive>`, `/mnt/<drive>`.
fn is_drive_mount(m: &Path) -> bool {
    let depth = |prefix: &str| m.strip_prefix(prefix).ok().map(|r| r.components().count());
    depth("/media") == Some(2) || depth("/run/media") == Some(2) || depth("/mnt") == Some(1)
}

/// Real filesystems people store files on (not snaps, containers or kernel views).
const USER_FS: &[&str] = &["ext4", "ext3", "ext2", "btrfs", "xfs", "f2fs", "zfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "hfsplus", "apfs"];

/// Zenity or kdialog, whichever the desktop has (GNOME and most others ship zenity; KDE ships kdialog).
enum Dialog {
    Zenity,
    Kdialog,
}

fn dialog() -> Option<Dialog> {
    let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_uppercase().contains("KDE"));
    match (has("zenity"), has("kdialog")) {
        (_, true) if kde => Some(Dialog::Kdialog),
        (true, _) => Some(Dialog::Zenity),
        (false, true) => Some(Dialog::Kdialog),
        _ => None,
    }
}

fn pick(save: Option<&str>, folders: bool, title: &str) -> Option<PathBuf> {
    let home = super::home_dir();
    let mut c = match dialog()? {
        Dialog::Zenity => {
            let mut c = Command::new("zenity");
            c.args(["--file-selection", "--title", title]);
            if folders {
                c.arg("--directory");
            }
            if let Some(name) = save {
                c.args(["--save", "--confirm-overwrite", &format!("--filename={}", home.join(name).display())]);
            }
            c
        }
        Dialog::Kdialog => {
            let mut c = Command::new("kdialog");
            c.args(["--title", title]);
            match (save, folders) {
                (Some(name), _) => c.arg("--getsavefilename").arg(home.join(name)),
                (None, true) => c.arg("--getexistingdirectory").arg(&home),
                (None, false) => c.arg("--getopenfilename").arg(&home),
            };
            c
        }
    };
    output(&mut c).filter(|s| !s.is_empty()).map(PathBuf::from)
}

impl Platform for Linux {
    fn memory_extra(&self) -> Option<MemExtra> {
        let m = meminfo();
        let total = *m.get("MemTotal")?;
        let available = *m.get("MemAvailable")?;
        let used = total.saturating_sub(available);
        let cached = m.get("Cached").copied().unwrap_or(0) + m.get("Buffers").copied().unwrap_or(0);
        let wired = m.get("Unevictable").copied().unwrap_or(0) + m.get("Mlocked").copied().unwrap_or(0);
        let swap_total = m.get("SwapTotal").copied().unwrap_or(0);
        let swap_used = swap_total.saturating_sub(m.get("SwapFree").copied().unwrap_or(0));
        // the kernel's own stall figure where it has one; otherwise how much memory is left
        let pressure = match memory_pressure() {
            Some(p) if p >= 10.0 => Pressure::Critical,
            Some(p) if p >= 2.0 => Pressure::Warn,
            Some(_) => Pressure::Normal,
            None => match available * 100 / total.max(1) {
                0..=5 => Pressure::Critical,
                6..=12 => Pressure::Warn,
                _ => Pressure::Normal,
            },
        };
        Some(MemExtra { app: used.saturating_sub(wired), wired, compressed: m.get("Zswap").copied().unwrap_or(0), cached, used, pressure, swap_used, swap_total })
    }

    fn available_for_important_usage(&self, _: &Path) -> Option<u64> {
        None // Linux has no purgeable space
    }

    fn is_user_volume(&self, mount: &Path, fs: &str) -> bool {
        let real = USER_FS.contains(&fs.to_lowercase().as_str());
        real && (mount == Path::new("/") || mount.starts_with("/home") || is_drive_mount(mount))
    }

    fn volume_label(&self, mount: &Path, fallback: &str) -> String {
        if mount == Path::new("/") {
            return "Computer".into();
        }
        if mount == Path::new("/home") {
            return "Home".into();
        }
        mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| fallback.to_string())
    }

    /// The file manager's own "show this item" (the FileManager1 D-Bus service Nautilus, Dolphin, Nemo and
    /// others provide); the folder opened plainly when there is none.
    fn reveal(&self, path: &Path) -> io::Result<()> {
        if has("gdbus") {
            let ok = Command::new("gdbus")
                .args(["call", "--session", "--dest", "org.freedesktop.FileManager1", "--object-path", "/org/freedesktop/FileManager1", "--method", "org.freedesktop.FileManager1.ShowItems"])
                .arg(format!("[{}]", gv(&file_uri(path))))
                .arg("''")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                return Ok(());
            }
        }
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        self.open(dir)
    }

    fn open(&self, path: &Path) -> io::Result<()> {
        spawn(Command::new("xdg-open").arg(path))
    }

    fn quick_look(&self, path: &Path) -> io::Result<()> {
        self.open(path) // no Quick Look: the default app opens it
    }

    fn open_full_disk_access_settings(&self) -> io::Result<()> {
        Ok(()) // Linux has no such permission
    }

    fn full_disk_access(&self) -> Option<bool> {
        None
    }

    /// The desktop's notification service (org.freedesktop.Notifications), through `notify-send` when present.
    fn notify(&self, title: &str, body: &str) {
        let _ = self.notify_checked(title, body);
    }

    fn notify_checked(&self, title: &str, body: &str) -> Result<(), String> {
        let ok = if has("notify-send") {
            Command::new("notify-send").args(["--app-name=Deskveet", "--icon=deskveet", title, body]).status().is_ok_and(|s| s.success())
        } else {
            Command::new("gdbus")
                .args(["call", "--session", "--dest", "org.freedesktop.Notifications", "--object-path", "/org/freedesktop/Notifications", "--method", "org.freedesktop.Notifications.Notify"])
                .args([gv("Deskveet"), "0".into(), gv("deskveet"), gv(title), gv(body), "[]".into(), "{}".into(), "-1".into()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if ok { Ok(()) } else { Err("The desktop did not show the notification.".into()) }
    }

    /// A question with Cancel as the default answer.
    fn confirm(&self, title: &str, message: &str, ok: &str) -> bool {
        let mut c = match dialog() {
            Some(Dialog::Zenity) => {
                let mut c = Command::new("zenity");
                c.args(["--question", "--default-cancel", "--no-wrap", "--title", title, "--text", message, "--ok-label", ok, "--cancel-label", "Cancel"]);
                c
            }
            Some(Dialog::Kdialog) => {
                let mut c = Command::new("kdialog");
                c.args(["--title", title, "--warningcontinuecancel", message, "--continue-label", ok]);
                c
            }
            None => return false,
        };
        c.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    /// Ticked items are kept; an empty answer keeps none.
    fn choose_from_list(&self, title: &str, prompt: &str, items: &[String], ok: &str) -> Option<Vec<String>> {
        let mut c = match dialog()? {
            Dialog::Zenity => {
                let mut c = Command::new("zenity");
                c.args(["--list", "--checklist", "--separator=\n", "--title", title, "--text", prompt, "--ok-label", ok, "--column", "Keep", "--column", "App"]);
                for i in items {
                    c.args(["FALSE", i]);
                }
                c
            }
            Dialog::Kdialog => {
                let mut c = Command::new("kdialog");
                c.args(["--title", title, "--separate-output", "--checklist", prompt]);
                for i in items {
                    c.args([i.as_str(), i.as_str(), "off"]);
                }
                c
            }
        };
        let out = c.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(String::from).collect())
    }

    /// The desktop's own list of startup apps lives in a folder; it is opened in the file manager.
    fn open_login_items_settings(&self) -> io::Result<()> {
        let dir = super::home_dir().join(".config/autostart");
        let _ = std::fs::create_dir_all(&dir);
        self.open(&dir)
    }

    fn sleep_now(&self) -> io::Result<()> {
        spawn(Command::new("systemctl").arg("suspend"))
    }

    fn lock_screen(&self) -> io::Result<()> {
        if has("loginctl") && Command::new("loginctl").arg("lock-session").status().is_ok_and(|s| s.success()) {
            return Ok(());
        }
        spawn(Command::new("xdg-screensaver").arg("lock"))
    }

    /// Through systemd-logind, which tells running apps the session is ending.
    fn power(&self, what: Power) -> Result<(), String> {
        let verb = match what {
            Power::Restart => "reboot",
            Power::ShutDown => "poweroff",
        };
        let ok = Command::new("systemctl").arg(verb).status().map_err(|e| e.to_string())?;
        if ok.success() { Ok(()) } else { Err("The system refused.".into()) }
    }

    /// Drives the desktop mounted for this user (USB sticks, cards, external disks, disk images).
    fn ejectable_disks(&self) -> Vec<Disk> {
        mounts()
            .into_iter()
            .filter(|(dev, m, _)| dev.starts_with("/dev/") && is_drive_mount(m))
            .map(|(_, m, _)| Disk { name: m.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), path: m })
            .collect()
    }

    /// `gio mount -e` (what the file manager's Eject does), or udisks directly.
    fn eject(&self, disk: &Disk) -> Result<(), String> {
        let ok = if has("gio") {
            Command::new("gio").args(["mount", "-e"]).arg(&disk.path).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        } else if let Some((dev, _, _)) = mounts().into_iter().find(|(_, m, _)| *m == disk.path) {
            Command::new("udisksctl").args(["unmount", "-b", &dev]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        } else {
            false
        };
        if ok && !mounts().iter().any(|(_, m, _)| *m == disk.path) {
            Ok(())
        } else {
            Err(format!("{} is still in use.", disk.name))
        }
    }

    /// `systemd-inhibit` holding off sleep (and the screen blanking) until it is killed, the time runs out,
    /// or this app exits.
    fn keep_awake(&self, display: bool, secs: Option<u64>) -> io::Result<std::process::Child> {
        let me = std::process::id();
        let what = if display { "idle:sleep" } else { "sleep" };
        let wait = format!("while kill -0 {me} 2>/dev/null; do sleep 5; done");
        let mut c = Command::new("systemd-inhibit");
        c.args([&format!("--what={what}"), "--who=Deskveet", "--why=Keep awake", "--mode=block"]);
        match secs {
            Some(s) => c.args(["timeout", &s.to_string(), "sh", "-c", &wait]),
            None => c.args(["sh", "-c", &wait]),
        };
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    }

    fn open_notification_settings(&self) -> io::Result<()> {
        let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_uppercase().contains("GNOME"));
        if gnome && has("gnome-control-center") {
            return spawn(Command::new("gnome-control-center").arg("notifications"));
        }
        if has("systemsettings") {
            return spawn(Command::new("systemsettings").arg("kcm_notifications"));
        }
        Ok(())
    }

    fn pick_save_file(&self, default_name: &str) -> Option<PathBuf> {
        pick(Some(default_name), false, "Save")
    }

    fn pick_file(&self) -> Option<PathBuf> {
        pick(None, false, "Choose a file")
    }

    fn pick_folder(&self) -> Option<PathBuf> {
        pick(None, true, "Choose a folder")
    }
}

/// Processes that keep the desktop and the system running.
const PROTECTED_NAMES: &[&str] = &[
    "systemd", "init", "kthreadd", "dbus-daemon", "dbus-broker", "pipewire", "pipewire-pulse", "wireplumber", "pulseaudio", "Xorg", "Xwayland",
    "gnome-shell", "gnome-session-binary", "plasmashell", "kwin_x11", "kwin_wayland", "ksmserver", "xfce4-session", "xfwm4", "cinnamon", "mutter",
    "gsd-xsettings", "polkitd", "NetworkManager", "sshd", "login", "gdm", "sddm", "lightdm", "at-spi-bus-launcher", "xdg-desktop-portal",
    "xdg-document-portal", "xdg-permission-store", "gvfsd", "ibus-daemon", "fcitx5",
];

impl ProcessInfo for Linux {
    fn protected_reason(&self, p: &ProcFacts, me_uid: u32, me_pid: u32) -> Option<&'static str> {
        if p.pid <= 2 {
            return Some("system process");
        }
        if p.pid == me_pid {
            return Some("this app");
        }
        if p.uid != me_uid {
            return Some("owned by another user or by the system");
        }
        if PROTECTED_NAMES.contains(&p.name) {
            return Some("part of the desktop");
        }
        None
    }

    fn reveal_target(&self, exe: &Path) -> PathBuf {
        exe.to_path_buf()
    }
}

/// Folders shared by many programs: a program there is grouped by its own path, not its folder.
const SHARED_DIRS: &[&str] = &["/usr/bin", "/usr/sbin", "/bin", "/sbin", "/usr/local/bin", "/usr/libexec", "/usr/lib", "/usr/lib64", "/snap/bin"];

impl Apps for Linux {
    /// The desktop apps this user is running: processes whose program is the one an installed app
    /// (a `.desktop` entry) starts, one row per app.
    fn running_apps(&self) -> Vec<RunningApp> {
        let apps = crate::linuxapps::desktop_entries();
        let uid = unsafe { libc::getuid() };
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else { return out };
        let mut pids: Vec<u32> = dir.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
        pids.sort_unstable(); // the first process of an app is usually its main one
        for pid in pids {
            let proc_dir = PathBuf::from(format!("/proc/{pid}"));
            if std::fs::metadata(&proc_dir).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).ok() != Some(uid) {
                continue;
            }
            let Ok(exe) = std::fs::read_link(proc_dir.join("exe")) else { continue };
            let comm = std::fs::read_to_string(proc_dir.join("comm")).unwrap_or_default().trim().to_string();
            let Some(app) = apps.iter().find(|a| a.runs_as(&exe, &comm)) else { continue };
            if !seen.insert(app.id.clone()) {
                continue;
            }
            let folder = exe.parent().filter(|d| !SHARED_DIRS.iter().any(|s| *d == Path::new(s))).map(Path::to_path_buf);
            out.push(RunningApp { pid, name: app.name.clone(), bundle_id: app.id.clone(), bundle_path: folder });
        }
        out
    }

    fn app_icon_png(&self, _: &Path) -> Option<Vec<u8>> {
        None
    }

    /// SIGTERM lets the app close the way its window's close button would; SIGKILL ends it at once.
    fn quit_app(&self, pid: u32, force: bool) -> bool {
        // SAFETY: plain signal to a process id.
        unsafe { libc::kill(pid as i32, if force { libc::SIGKILL } else { libc::SIGTERM }) == 0 }
    }

    fn responsible_pid(&self, _: u32) -> Option<u32> {
        None
    }
}

/// Whether Deskveet starts when the user logs in (its entry in `~/.config/autostart`).
pub fn opens_at_login() -> bool {
    std::fs::read_to_string(autostart_file()).is_ok_and(|s| !s.lines().any(|l| l.trim() == "Hidden=true"))
}

fn autostart_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| super::home_dir().join(".config")).join("autostart/deskveet.desktop")
}

/// Add or remove Deskveet's autostart entry (the standard every desktop reads). An AppImage is started from
/// its own file, not from the folder it is unpacked into while running.
pub fn set_open_at_login(on: bool) -> Result<(), String> {
    let f = autostart_file();
    if !on {
        return match std::fs::remove_file(&f) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok()).ok_or("Could not tell where Deskveet is")?;
    std::fs::create_dir_all(f.parent().unwrap()).map_err(|e| e.to_string())?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Deskveet\nComment=Disk cleaner\nExec=\"{}\"\nIcon=deskveet\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&f, entry).map_err(|e| e.to_string())
}
