//! Linux apps: the installed list (desktop entries, including Flatpak and Snap apps), removing an app the way
//! it was installed, data that no installed app owns, and what starts when you log in.
//!
//! Parsing and matching are plain functions so they are tested on every platform.

use crate::fsutil;
use crate::inventory::{DataItem, LeftoverGroup, StartupItem};
use crate::safety::{Allowed, Safety};
use std::path::{Path, PathBuf};

/// How an app was installed, which decides how it is removed.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A Flatpak app; `user` installs need no administrator.
    Flatpak { app: String, user: bool },
    Snap(String),
    /// From the system's packages (apt, dnf, pacman, zypper).
    Package,
    /// A `.desktop` file the user added (an AppImage, a program unpacked by hand).
    Local,
}

/// One app from a `.desktop` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopApp {
    /// The desktop file id (`org.gnome.Nautilus.desktop`): stable, and how an uninstall is looked up again.
    pub id: String,
    pub name: String,
    pub file: PathBuf,
    /// The program it starts (first word of `Exec`, without the arguments).
    pub exec: Option<String>,
    pub wm_class: Option<String>,
    pub source: Source,
}

impl DesktopApp {
    /// Whether a process (`exe` is `/proc/<pid>/exe`, `comm` its short name) is this app running.
    pub fn runs_as(&self, exe: &Path, comm: &str) -> bool {
        match &self.source {
            Source::Snap(n) => exe.starts_with(format!("/snap/{n}/")),
            Source::Flatpak { app, .. } => comm.eq_ignore_ascii_case(app.rsplit('.').next().unwrap_or(app)),
            _ => {
                let base = |s: &str| Path::new(s).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let exe_name = exe.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                self.exec.as_deref().is_some_and(|e| (e.starts_with('/') && Path::new(e) == exe) || base(e) == exe_name || base(e) == comm)
                    || self.wm_class.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(comm))
            }
        }
    }

    /// The command that removes it, when that needs an administrator (it is shown, never run).
    pub fn remove_command(&self) -> Option<String> {
        match &self.source {
            Source::Snap(n) => Some(format!("sudo snap remove {n}")),
            Source::Package => package_of(&self.file).map(|(tool, pkg)| match tool {
                "dpkg" => format!("sudo apt remove {pkg}"),
                "rpm" => format!("sudo dnf remove {pkg}"),
                "pacman" => format!("sudo pacman -R {pkg}"),
                _ => format!("sudo {tool} remove {pkg}"),
            }),
            _ => None,
        }
    }
}

/// The value of `key` in the `[Desktop Entry]` group (unlocalized).
fn entry_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut in_entry = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

/// The program an `Exec` line starts: quotes removed, `env VAR=x` and field codes skipped.
pub fn exec_program(exec: &str) -> Option<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in exec.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let mut it = words.into_iter().peekable();
    if it.peek().is_some_and(|w| w == "env" || w.ends_with("/env")) {
        it.next();
    }
    it.find(|w| !w.contains('=') && !w.starts_with('%'))
}

/// Parse one `.desktop` file. Hidden entries, links and entries for other desktops' settings are left out.
pub fn parse_entry(id: &str, file: &Path, text: &str, home: &Path) -> Option<DesktopApp> {
    if entry_value(text, "Type") != Some("Application") || entry_value(text, "NoDisplay") == Some("true") || entry_value(text, "Hidden") == Some("true") {
        return None;
    }
    let name = entry_value(text, "Name")?.to_string();
    let exec_line = entry_value(text, "Exec").unwrap_or("");
    let f = file.to_string_lossy();
    let source = if let Some(app) = entry_value(text, "X-Flatpak") {
        Source::Flatpak { app: app.to_string(), user: file.starts_with(home) }
    } else if f.contains("/snapd/desktop/applications") {
        Source::Snap(entry_value(text, "X-SnapInstanceName").map(String::from).unwrap_or_else(|| id.split('_').next().unwrap_or(id).to_string()))
    } else if file.starts_with(home) {
        Source::Local
    } else {
        Source::Package
    };
    Some(DesktopApp { id: id.to_string(), name, file: file.to_path_buf(), exec: exec_program(exec_line), wm_class: entry_value(text, "StartupWMClass").map(String::from), source })
}

/// Where desktop entries live, most specific first (a user's entry hides the system one with the same id).
fn entry_dirs(home: &Path) -> Vec<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    let mut v = vec![data_home.join("applications"), data_home.join("flatpak/exports/share/applications")];
    v.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("applications")));
    v.extend([PathBuf::from("/var/lib/flatpak/exports/share/applications"), PathBuf::from("/var/lib/snapd/desktop/applications")]);
    v.dedup();
    v
}

/// Every app with a desktop entry, by name.
pub fn desktop_entries() -> Vec<DesktopApp> {
    let home = crate::platform::home_dir();
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in entry_dirs(&home) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if !id.ends_with(".desktop") || !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            if let Some(a) = parse_entry(&id, &e.path(), &text, &home) {
                apps.push(a);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// The system package that installed `file`, asking whichever package manager the system has.
fn package_of(file: &Path) -> Option<(&'static str, String)> {
    let run = |tool: &str, args: &[&str]| -> Option<String> {
        let out = std::process::Command::new(tool).args(args).arg(file).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    if let Some(s) = run("dpkg", &["-S"]) {
        return s.split(':').next().map(|p| ("dpkg", p.trim().to_string()));
    }
    if let Some(s) = run("rpm", &["-qf", "--qf", "%{NAME}"]) {
        return Some(("rpm", s));
    }
    if let Some(s) = run("pacman", &["-Qqo"]) {
        return Some(("pacman", s));
    }
    None
}

/// Apps that are part of the desktop or this app: listed, never offered for removal.
pub fn locked_reason(a: &DesktopApp) -> Option<&'static str> {
    let id = a.id.to_lowercase();
    if id.contains("deskveet") {
        return Some("This app");
    }
    let desktop = ["org.gnome.", "org.kde.", "org.freedesktop.", "xfce", "cinnamon", "mate-", "gnome-", "kde-", "plasma", "nautilus", "dolphin", "konsole", "systemsettings", "yelp", "software-properties", "update-manager"];
    if desktop.iter().any(|d| id.starts_with(d)) {
        return Some("Part of the desktop");
    }
    if a.source == Source::Local {
        return Some("Added by hand: remove it where you put it");
    }
    None
}

/// Remove an app that needs no administrator: a Flatpak (the system's own prompt appears for apps installed
/// for all users). Blocks until it finishes.
pub fn uninstall(a: &DesktopApp) -> Result<(), String> {
    match &a.source {
        Source::Flatpak { app, user } => {
            let st = std::process::Command::new("flatpak")
                .args(["uninstall", "--noninteractive", if *user { "--user" } else { "--system" }, app])
                .status()
                .map_err(|e| e.to_string())?;
            if st.success() { Ok(()) } else { Err(format!("Flatpak could not remove {}.", a.name)) }
        }
        _ => Err(match a.remove_command() {
            Some(c) => format!("Run this in a terminal to remove it: {c}"),
            None => "Remove it the way it was installed.".into(),
        }),
    }
}

/// Lower-case letters and digits only, for loose name matching.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Folders that belong to the desktop, to the tools Deskveet's rules already cover, or to things that are not
/// apps. Never offered as leftovers.
const KEEP: &[&str] = &[
    "autostart", "dconf", "gtk20", "gtk30", "gtk40", "pulse", "systemd", "fontconfig", "fonts", "icons", "themes", "applications", "mime",
    "trash", "keyrings", "kwalletd", "recentlyused", "gvfsmetadata", "tracker", "tracker3", "flatpak", "snap", "nautilus", "dolphin",
    "gnome", "gnomeshell", "gnomesession", "gnomesettingsdaemon", "goa10", "evolution", "ibus", "fcitx", "fcitx5", "kde", "kdeconnect",
    "plasma", "kwin", "xfce4", "cinnamon", "nemo", "thumbnails", "mesashadercache", "fontconfig", "pipewire", "wireplumber",
    "menus", "desktopdirectories", "xorg", "sessions", "deskveet", "appdeskveetdesktop", "npm", "yarn", "pip", "pnpm", "gobuild",
    "mozilla", "googlechrome", "chromium", "bravesoftware", "microsoftedge", "code", "discord", "slack", "spotify", "jetbrains",
    "thunderbird", "libreoffice", "user", "nvidia", "vlc", "gimp20", "ssh", "gnupg", "containers", "docker", "dbus1",
];

fn owned(folder: &str, apps: &[DesktopApp]) -> bool {
    let f = norm(folder);
    if f.len() < 3 || KEEP.contains(&f.as_str()) {
        return true;
    }
    apps.iter().any(|a| {
        let exec = a.exec.as_deref().and_then(|e| Path::new(e).file_name()).map(|n| norm(&n.to_string_lossy())).unwrap_or_default();
        let id = norm(a.id.trim_end_matches(".desktop"));
        let last = a.id.trim_end_matches(".desktop").rsplit('.').next().map(norm).unwrap_or_default();
        [norm(&a.name), exec, id, last].iter().any(|n| n.len() >= 3 && (n.contains(&f) || f.contains(n.as_str())))
    })
}

/// Folders in the config, data and cache folders, and Flatpak app data (`~/.var/app`), that no installed app
/// owns and that nothing has changed for a long time. These are guesses, so the page marks them for review.
pub fn leftovers(home: &Path, apps: &[DesktopApp], safety: &Safety, changed_before: i64) -> Vec<LeftoverGroup> {
    let xdg = |var: &str, rel: &str| std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home.join(rel));
    let roots = [
        (xdg("XDG_CONFIG_HOME", ".config"), "Settings"),
        (xdg("XDG_DATA_HOME", ".local/share"), "App data"),
        (xdg("XDG_CACHE_HOME", ".cache"), "Caches"),
        (home.join(".var/app"), "Flatpak app data"),
    ];
    leftovers_in(&roots, apps, safety, changed_before)
}

/// [`leftovers`] in the given folders, each with the kind of data it holds.
pub fn leftovers_in(roots: &[(PathBuf, &'static str)], apps: &[DesktopApp], safety: &Safety, changed_before: i64) -> Vec<LeftoverGroup> {
    let flatpaks: Vec<&str> = apps.iter().filter_map(|a| if let Source::Flatpak { app, .. } = &a.source { Some(app.as_str()) } else { None }).collect();
    let mut groups: Vec<LeftoverGroup> = Vec::new();
    for (root, kind) in roots {
        let (root, kind) = (root, *kind);
        let flatpak_root = kind == "Flatpak app data";
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            // Flatpak data is named after the app id exactly; everything else is matched loosely
            let has_app = if flatpak_root { flatpaks.contains(&name.as_str()) } else { owned(&name, apps) };
            if !e.file_type().is_ok_and(|t| t.is_dir()) || name.starts_with('.') || has_app || safety.check(&p, &Allowed::UserPick).is_err() {
                continue;
            }
            let m = fsutil::measure(&p);
            if m.mtime > changed_before || m.bytes == 0 {
                continue;
            }
            let item = DataItem { path: p.to_string_lossy().into_owned(), kind: kind.to_string(), size: m.bytes, mtime: m.mtime };
            match groups.iter_mut().find(|g| norm(&g.id) == norm(&name)) {
                Some(g) => {
                    g.total += item.size;
                    g.newest = g.newest.max(item.mtime);
                    g.items.push(item);
                }
                None => groups.push(LeftoverGroup { id: name, total: item.size, newest: item.mtime, items: vec![item] }),
            }
        }
    }
    groups.sort_by(|a, b| b.total.cmp(&a.total));
    groups
}

/// Autostart entries: the user's (`~/.config/autostart`) and the system's (`/etc/xdg/autostart`), where a user
/// entry with the same name replaces the system one.
pub fn startup_rows(home: &Path, running_comms: &[String]) -> Vec<StartupItem> {
    let user = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("autostart");
    startup_rows_in(&user, Path::new("/etc/xdg/autostart"), running_comms)
}

/// [`startup_rows`] from the given user and system autostart folders.
pub fn startup_rows_in(user: &Path, system: &Path, running_comms: &[String]) -> Vec<StartupItem> {
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for (dir, all) in [(user, false), (system, true)] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if !file.ends_with(".desktop") || !seen.insert(file.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            // entries for one desktop only (OnlyShowIn=KDE;) still count: they start there
            let name = entry_value(&text, "Name").unwrap_or(file.trim_end_matches(".desktop")).to_string();
            let program = entry_value(&text, "Exec").and_then(exec_program);
            let disabled = entry_value(&text, "Hidden") == Some("true") || entry_value(&text, "X-GNOME-Autostart-enabled") == Some("false");
            let base = program.as_deref().and_then(|p| Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            rows.push(StartupItem {
                label: file,
                plist: e.path().to_string_lossy().into_owned(),
                running: !base.is_empty() && running_comms.iter().any(|c| base.starts_with(c.as_str()) || c == &base),
                program,
                owner: name,
                scope: if all { "All users" } else { "Your account" },
                run_at_load: true,
                keep_alive: false,
                disabled,
            });
        }
    }
    rows.sort_by(|a, b| a.owner.to_lowercase().cmp(&b.owner.to_lowercase()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entries_are_read_like_the_desktop_reads_them() {
        let home = Path::new("/home/a");
        let text = "[Desktop Entry]\nType=Application\nName=Visual Studio Code\nName[de]=Code\nExec=env GDK_BACKEND=x11 /usr/share/code/code --unity-launch %F\nStartupWMClass=Code\n[Desktop Action new]\nName=New Window\nExec=code -n\n";
        let a = parse_entry("code.desktop", Path::new("/usr/share/applications/code.desktop"), text, home).unwrap();
        assert_eq!((a.name.as_str(), a.exec.as_deref(), a.source.clone()), ("Visual Studio Code", Some("/usr/share/code/code"), Source::Package));
        assert!(a.runs_as(Path::new("/usr/share/code/code"), "code"));
        assert!(!a.runs_as(Path::new("/usr/bin/vim"), "vim"));

        let fp = "[Desktop Entry]\nType=Application\nName=Spotify\nExec=/usr/bin/flatpak run --branch=stable com.spotify.Client\nX-Flatpak=com.spotify.Client\n";
        let f = parse_entry("com.spotify.Client.desktop", Path::new("/home/a/.local/share/flatpak/exports/share/applications/x.desktop"), fp, home).unwrap();
        assert_eq!(f.source, Source::Flatpak { app: "com.spotify.Client".into(), user: true });
        assert_eq!(f.remove_command(), None, "user Flatpaks are removed without an administrator");

        let hidden = "[Desktop Entry]\nType=Application\nName=Helper\nNoDisplay=true\nExec=helper\n";
        assert!(parse_entry("h.desktop", Path::new("/usr/share/applications/h.desktop"), hidden, home).is_none());
        let snap = "[Desktop Entry]\nType=Application\nName=Slack\nExec=/snap/bin/slack %U\nX-SnapInstanceName=slack\n";
        let s = parse_entry("slack_slack.desktop", Path::new("/var/lib/snapd/desktop/applications/slack_slack.desktop"), snap, home).unwrap();
        assert_eq!(s.remove_command().as_deref(), Some("sudo snap remove slack"));
        assert!(s.runs_as(Path::new("/snap/slack/123/usr/lib/slack/slack"), "slack"));
    }

    #[test]
    fn exec_lines_name_their_program() {
        assert_eq!(exec_program("\"/opt/My App/app\" --x %U").as_deref(), Some("/opt/My App/app"));
        assert_eq!(exec_program("env A=1 B=2 firefox %u").as_deref(), Some("firefox"));
        assert_eq!(exec_program("").as_deref(), None);
    }

    #[test]
    fn leftovers_are_unowned_idle_folders_and_never_desktop_ones() {
        let t = tempfile::tempdir().unwrap();
        let home = crate::safety::canon(t.path()).unwrap();
        for d in [".config/OldEditor", ".config/autostart", ".config/Code", ".local/share/OldEditor", ".var/app/org.gone.App", ".var/app/com.spotify.Client", ".cache/fresh-tool"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
            std::fs::write(home.join(d).join("f"), vec![1u8; 4096]).unwrap();
        }
        let old = crate::scan::now_secs() as i64 - 200 * 86_400;
        for d in [".config/OldEditor", ".config/autostart", ".config/Code", ".local/share/OldEditor", ".var/app/org.gone.App", ".var/app/com.spotify.Client"] {
            crate::testutil::set_mtime(&home.join(d).join("f"), old as u64);
            crate::testutil::set_mtime(&home.join(d), old as u64);
        }
        let apps = vec![
            DesktopApp { id: "code.desktop".into(), name: "Visual Studio Code".into(), file: "/usr/share/applications/code.desktop".into(), exec: Some("/usr/share/code/code".into()), wm_class: None, source: Source::Package },
            DesktopApp { id: "com.spotify.Client.desktop".into(), name: "Spotify".into(), file: "/x".into(), exec: None, wm_class: None, source: Source::Flatpak { app: "com.spotify.Client".into(), user: true } },
        ];
        let safety = Safety::new(home.clone());
        // the folders are passed in: a test machine may set XDG_CONFIG_HOME and friends to its own folders
        let roots = [(home.join(".config"), "Settings"), (home.join(".local/share"), "App data"), (home.join(".cache"), "Caches"), (home.join(".var/app"), "Flatpak app data")];
        let groups = leftovers_in(&roots, &apps, &safety, crate::scan::now_secs() as i64 - 90 * 86_400);
        let mut ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["OldEditor", "org.gone.App"], "settings and data of one gone app are one group; desktop folders, installed apps and fresh data are left alone");
        assert_eq!(groups.iter().find(|g| g.id == "OldEditor").unwrap().items.len(), 2);
    }

    #[test]
    fn autostart_entries_say_whether_they_are_turned_off() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let dir = home.join(".config/autostart");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.desktop"), "[Desktop Entry]\nType=Application\nName=Syncer\nExec=/usr/bin/syncer --tray\n").unwrap();
        std::fs::write(dir.join("b.desktop"), "[Desktop Entry]\nType=Application\nName=Chat\nExec=chat\nX-GNOME-Autostart-enabled=false\n").unwrap();
        let rows = startup_rows_in(&dir, &home.join("no-system-entries"), &["syncer".to_string()]);
        let mine: Vec<_> = rows.iter().filter(|r| r.scope == "Your account").map(|r| (r.owner.as_str(), r.running, r.disabled)).collect();
        assert_eq!(mine, [("Chat", false, true), ("Syncer", true, false)]);
    }
}
