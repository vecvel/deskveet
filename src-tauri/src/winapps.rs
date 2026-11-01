//! Windows apps: the installed list (from the uninstall entries in the registry), running an app's own
//! uninstaller, data in AppData that no installed app owns, and what starts at sign-in.
//!
//! The parsing and matching here are plain functions so they are tested on every platform; only reading
//! the registry and starting programs is Windows-only.

use crate::fsutil;
use crate::inventory::{DataItem, LeftoverGroup, StartupItem};
use crate::safety::{Allowed, Safety};
use std::path::{Path, PathBuf};

/// One entry of Windows' installed programs list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WinApp {
    /// Where the entry lives (`HKCU\...\Uninstall\<key>`): stable, and how an uninstall is looked up again.
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub install_dir: Option<PathBuf>,
    /// The command Windows' own Settings runs to uninstall it.
    pub uninstall: Option<String>,
    /// Bytes, as the app reported them when it was installed.
    pub size: Option<u64>,
    /// Installed for all users: its uninstaller will ask Windows for permission.
    pub all_users: bool,
}

/// Apps that other apps or Windows itself rely on: listed, never offered for uninstalling here.
pub fn locked_reason(a: &WinApp) -> Option<&'static str> {
    let n = a.name.to_lowercase();
    if n.contains("deskveet") {
        return Some("This app");
    }
    let shared = ["visual c++", ".net", "webview2", "microsoft edge", "windows sdk", "windows software development kit", "directx", "vcredist", "microsoft update health"];
    if shared.iter().any(|s| n.contains(s)) {
        return Some("Used by Windows or other apps");
    }
    if a.uninstall.as_deref().map_or(true, |u| u.trim().is_empty()) {
        return Some("Has no uninstaller");
    }
    None
}

/// Split an uninstall command line into a program and its arguments, the way Windows would run it, without
/// a shell: `"C:\App\unins000.exe" /SILENT`, `C:\App\uninstall.exe`, `MsiExec.exe /X{GUID}`.
pub fn split_command(cmd: &str) -> Option<(String, Vec<String>)> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    let (prog, rest) = if let Some(r) = cmd.strip_prefix('"') {
        let end = r.find('"')?;
        (r[..end].to_string(), r[end + 1..].trim())
    } else if let Some(i) = cmd.to_lowercase().find(".exe") {
        // unquoted paths may contain spaces; the program ends at its .exe
        (cmd[..i + 4].to_string(), cmd[i + 4..].trim())
    } else {
        let mut it = cmd.splitn(2, ' ');
        (it.next()?.to_string(), it.next().unwrap_or("").trim())
    };
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in rest.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        args.push(cur);
    }
    // Windows Installer entries often say /I (repair/change); the uninstall is /X
    if Path::new(&prog).file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case("msiexec.exe") || f.to_string_lossy().eq_ignore_ascii_case("msiexec")) {
        for a in &mut args {
            if a.len() > 2 && (a.starts_with("/I") || a.starts_with("/i")) && a[2..].starts_with('{') {
                *a = format!("/X{}", &a[2..]);
            }
        }
    }
    Some((prog, args))
}

/// Lower-case letters and digits only, for loose name matching (`Visual Studio Code` ~ `Code`, `JetBrains`).
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Folders in AppData that belong to Windows, to the tools Deskveet's rules already cover, or to things that
/// are not apps. Never offered as leftovers.
const KEEP: &[&str] = &[
    "microsoft", "packages", "temp", "programs", "comms", "connecteddevicesplatform", "crashdumps", "d3dscache", "publishers",
    "virtualstore", "microsoftedge", "history", "fontcache", "iconcache", "elevateddiagnostics", "deskveet", "appdeskveetdesktop",
    "npmcache", "npm", "pip", "yarn", "pnpm", "gobuild", "pub", "nuget", "google", "mozilla", "bravesoftware", "nvidia", "nvidiacorporation",
    "amd", "intel", "apple", "applecomputer", "adobe", "squirreltemp", "sun", "oracle", "jetbrains", "android", "github", "code", "discord",
    "slack", "spotify", "zoom", "teams", "onedrive", "steam", "epicgameslauncher", "unity", "docker", "dockerdesktop", "windowsapps",
];

/// What `id` matches among installed apps (by name, publisher or install folder), loosely.
fn owned(folder: &str, apps: &[WinApp]) -> bool {
    let f = norm(folder);
    if f.len() < 3 || KEEP.contains(&f.as_str()) {
        return true; // too short to judge, or never ours to offer
    }
    apps.iter().any(|a| {
        let dir = a.install_dir.as_ref().and_then(|d| d.file_name()).map(|n| norm(&n.to_string_lossy())).unwrap_or_default();
        [norm(&a.name), norm(&a.publisher), dir].iter().any(|n| n.len() >= 3 && (n.contains(&f) || f.contains(n.as_str())))
    })
}

/// Folders directly in `Local`, `Roaming` and `LocalLow` that no installed app owns and that nothing has
/// changed for `idle_days`. These are guesses, so the page marks them for review.
pub fn leftovers(home: &Path, apps: &[WinApp], safety: &Safety, changed_before: i64) -> Vec<LeftoverGroup> {
    let roots = [("AppData/Local", "Local app data"), ("AppData/Roaming", "Roaming app data"), ("AppData/LocalLow", "Low-integrity app data")];
    let mut groups: Vec<LeftoverGroup> = Vec::new();
    for (rel, kind) in roots {
        let Ok(rd) = std::fs::read_dir(home.join(rel)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || owned(&name, apps) || safety.check(&p, &Allowed::UserPick).is_err() {
                continue;
            }
            let m = fsutil::measure(&p);
            if m.mtime > changed_before || m.bytes == 0 {
                continue; // still in use, or empty
            }
            let item = DataItem { path: p.to_string_lossy().into_owned(), kind: kind.to_string(), size: m.bytes, mtime: m.mtime };
            match groups.iter_mut().find(|g| g.id.eq_ignore_ascii_case(&name)) {
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

/// A program that starts at sign-in, from a Run key or a Startup folder.
pub struct StartEntry {
    pub name: String,
    pub command: String,
    /// `HKCU\...\Run`, a shortcut's path, ...: where it is defined, for "Show".
    pub source: String,
    pub all_users: bool,
    pub disabled: bool,
}

/// Turn start entries into the rows the Startup tab shows. `running` holds lower-case executable paths.
pub fn startup_rows(entries: Vec<StartEntry>, running: &[String]) -> Vec<StartupItem> {
    let mut v: Vec<StartupItem> = entries
        .into_iter()
        .map(|e| {
            let program = split_command(&e.command).map(|(p, _)| p);
            let is_running = program.as_ref().is_some_and(|p| running.iter().any(|r| r.eq_ignore_ascii_case(p)));
            StartupItem {
                owner: e.name.clone(),
                label: e.name,
                plist: program.clone().filter(|p| Path::new(p).exists()).unwrap_or(e.source),
                program,
                scope: if e.all_users { "All users" } else { "Your account" },
                running: is_running,
                run_at_load: true,
                keep_alive: false,
                disabled: e.disabled,
            }
        })
        .collect();
    v.sort_by(|a, b| a.owner.to_lowercase().cmp(&b.owner.to_lowercase()));
    v
}

#[cfg(windows)]
pub use reg::{installed, run_uninstaller, start_entries};

/// Reading the registry and starting uninstallers (Windows only).
#[cfg(windows)]
mod reg {
    use super::{split_command, StartEntry, WinApp};
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle came from RegOpenKeyExW and is closed once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(root: HKEY, path: &str) -> Option<Key> {
        let mut h: HKEY = std::ptr::null_mut();
        let p = wide(path);
        // SAFETY: valid root, NUL-terminated path, out-pointer.
        (unsafe { RegOpenKeyExW(root, p.as_ptr(), 0, KEY_READ, &mut h) } == ERROR_SUCCESS).then_some(Key(h))
    }

    fn subkeys(k: &Key) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            // SAFETY: sized buffer; enumeration stops at the first error.
            let r = unsafe { RegEnumKeyExW(k.0, i, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }

    /// A string or number value (`None` when missing).
    fn value(k: &Key, name: &str) -> Option<String> {
        let n = wide(name);
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: first call asks for the size, second reads into a buffer of that size.
        unsafe {
            if RegQueryValueExW(k.0, n.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut len) != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u8; len as usize + 2];
            if RegQueryValueExW(k.0, n.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr(), &mut len) != ERROR_SUCCESS {
                return None;
            }
            if kind == REG_DWORD {
                return Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]).to_string());
            }
            let w: Vec<u16> = buf[..len as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            Some(String::from_utf16_lossy(&w))
        }
    }

    /// Every value of a key: (name, data as text, first byte of binary data).
    fn values(k: &Key) -> Vec<(String, String, Option<u8>)> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 512];
            let mut nlen = name.len() as u32;
            let mut data = [0u8; 4096];
            let mut dlen = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: sized buffers; enumeration stops at the first error.
            let r = unsafe { RegEnumValueW(k.0, i, name.as_mut_ptr(), &mut nlen, std::ptr::null(), &mut kind, data.as_mut_ptr(), &mut dlen) };
            if r != ERROR_SUCCESS {
                break;
            }
            let w: Vec<u16> = data[..dlen as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            out.push((String::from_utf16_lossy(&name[..nlen as usize]), String::from_utf16_lossy(&w), data.first().copied().filter(|_| dlen > 0)));
        }
        out
    }

    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINSTALL_32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

    /// Installed programs as Windows' own Settings lists them: entries with a name, not system components,
    /// not updates of something else.
    pub fn installed() -> Vec<WinApp> {
        let mut out: Vec<WinApp> = Vec::new();
        for (root, rname, path, all_users) in [
            (HKEY_CURRENT_USER, "HKCU", UNINSTALL, false),
            (HKEY_LOCAL_MACHINE, "HKLM", UNINSTALL, true),
            (HKEY_LOCAL_MACHINE, "HKLM", UNINSTALL_32, true),
        ] {
            let Some(k) = open(root, path) else { continue };
            for sub in subkeys(&k) {
                let Some(e) = open(k.0, &sub) else { continue };
                let Some(name) = value(&e, "DisplayName").filter(|n| !n.trim().is_empty()) else { continue };
                if value(&e, "SystemComponent").as_deref() == Some("1") || value(&e, "ParentKeyName").is_some() {
                    continue;
                }
                if value(&e, "ReleaseType").is_some_and(|t| t.contains("Update") || t.contains("Hotfix")) {
                    continue;
                }
                let app = WinApp {
                    id: format!(r"{rname}\{path}\{sub}"),
                    name,
                    version: value(&e, "DisplayVersion").unwrap_or_default(),
                    publisher: value(&e, "Publisher").unwrap_or_default(),
                    install_dir: value(&e, "InstallLocation").map(|s| s.trim_matches('"').trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from),
                    uninstall: value(&e, "UninstallString"),
                    size: value(&e, "EstimatedSize").and_then(|s| s.parse::<u64>().ok()).map(|kb| kb * 1024),
                    all_users,
                };
                if !out.iter().any(|o| o.name == app.name && o.version == app.version) {
                    out.push(app);
                }
            }
        }
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    /// Start the app's own uninstaller (directly, no shell) and wait for it to close. Windows asks for
    /// permission itself when the app was installed for all users.
    pub fn run_uninstaller(app: &WinApp) -> Result<(), String> {
        let cmd = app.uninstall.as_deref().ok_or("This app has no uninstaller.")?;
        let (prog, args) = split_command(cmd).ok_or("The uninstall command could not be read.")?;
        let status = std::process::Command::new(&prog).args(&args).status().map_err(|e| format!("Could not start the uninstaller: {e}"))?;
        status.success().then_some(()).ok_or_else(|| "The uninstaller was closed or did not finish.".to_string())
    }

    /// Programs set to start at sign-in: the Run keys and the Startup folders, with what Settings switched off.
    pub fn start_entries(home: &std::path::Path) -> Vec<StartEntry> {
        const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
        // Settings marks a switched-off entry with an odd first byte under StartupApproved
        let off = |root: HKEY, sub: &str, name: &str| -> bool {
            open(root, &format!(r"{APPROVED}\{sub}")).is_some_and(|k| values(&k).iter().any(|(n, _, b)| n.eq_ignore_ascii_case(name) && b.is_some_and(|b| b % 2 == 1)))
        };
        let mut out = Vec::new();
        for (root, rname, all_users) in [(HKEY_CURRENT_USER, "HKCU", false), (HKEY_LOCAL_MACHINE, "HKLM", true)] {
            if let Some(k) = open(root, RUN) {
                for (name, command, _) in values(&k) {
                    if name.is_empty() || command.is_empty() {
                        continue;
                    }
                    let disabled = off(root, "Run", &name);
                    out.push(StartEntry { name, command, source: format!(r"{rname}\{RUN}"), all_users, disabled });
                }
            }
        }
        let program_data = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        for (dir, all_users) in [
            (home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"), false),
            (PathBuf::from(program_data).join(r"Microsoft\Windows\Start Menu\Programs\StartUp"), true),
        ] {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let file = e.file_name().to_string_lossy().into_owned();
                if file.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let name = std::path::Path::new(&file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(file.clone());
                let disabled = off(if all_users { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER }, "StartupFolder", &file);
                out.push(StartEntry { name, command: e.path().to_string_lossy().into_owned(), source: e.path().to_string_lossy().into_owned(), all_users, disabled });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn uninstall_commands_split_like_windows_runs_them() {
        let s = |c: &str| split_command(c).unwrap();
        assert_eq!(s(r#""C:\Program Files\App\unins000.exe" /SILENT"#), (r"C:\Program Files\App\unins000.exe".into(), vec!["/SILENT".into()]));
        assert_eq!(s(r"C:\Program Files\Tool\uninstall.exe --mode gui"), (r"C:\Program Files\Tool\uninstall.exe".into(), vec!["--mode".to_string(), "gui".into()]));
        assert_eq!(s("MsiExec.exe /I{12345678-1234-1234-1234-123456789012}"), ("MsiExec.exe".into(), vec!["/X{12345678-1234-1234-1234-123456789012}".into()]), "Windows Installer: /I repairs, /X removes");
        assert_eq!(s(r#""C:\a b\u.exe" "/log=C:\x y\l.txt""#).1, vec![r"/log=C:\x y\l.txt".to_string()]);
        assert!(split_command("   ").is_none());
    }

    #[test]
    fn shared_runtimes_and_apps_without_uninstallers_are_locked() {
        let a = |name: &str, un: Option<&str>| WinApp { name: name.into(), uninstall: un.map(String::from), ..Default::default() };
        assert!(locked_reason(&a("Microsoft Visual C++ 2015-2022 Redistributable (x64)", Some("x.exe"))).is_some());
        assert!(locked_reason(&a("Microsoft Edge WebView2 Runtime", Some("x.exe"))).is_some());
        assert!(locked_reason(&a("Deskveet", Some("x.exe"))).is_some());
        assert!(locked_reason(&a("Some Tool", None)).is_some());
        assert_eq!(locked_reason(&a("VLC media player", Some(r#""C:\Program Files\VideoLAN\VLC\uninstall.exe""#))), None);
    }

    #[test]
    fn leftovers_are_unowned_unchanged_app_data_folders() {
        let t = tempfile::tempdir().unwrap();
        let home = crate::safety::canon(t.path()).unwrap().join("profile");
        let put = |rel: &str| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![1u8; 5_000]).unwrap();
        };
        for rel in ["AppData/Local/OldTool/cache.bin", "AppData/Roaming/OldTool/settings.json", "AppData/Local/VideoLAN/vlc.ini", "AppData/Local/Microsoft/x.bin", "AppData/Roaming/BusyApp/now.bin", "AppData/Local/Temp/t.tmp"] {
            put(rel);
        }
        let long_ago = 1_000_000;
        for rel in ["AppData/Local/OldTool/cache.bin", "AppData/Local/OldTool", "AppData/Roaming/OldTool/settings.json", "AppData/Roaming/OldTool", "AppData/Local/VideoLAN/vlc.ini", "AppData/Local/VideoLAN", "AppData/Local/Microsoft/x.bin", "AppData/Local/Microsoft"] {
            crate::testutil::set_mtime(&home.join(rel), long_ago);
        }
        let vlc = WinApp { name: "VLC media player".into(), publisher: "VideoLAN".into(), ..Default::default() };
        let g = leftovers(&home, &[vlc], &Safety::new(home.clone()), 2_000_000);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(g[0].id, "OldTool");
        assert_eq!(g[0].items.len(), 2, "Local and Roaming together");
        assert!(g[0].total >= 10_000);
    }

    #[test]
    fn startup_rows_name_the_program_and_whether_it_runs() {
        let rows = startup_rows(
            vec![
                StartEntry { name: "Tool".into(), command: r#""C:\Tools\tool.exe" --tray"#.into(), source: r"HKCU\Run".into(), all_users: false, disabled: false },
                StartEntry { name: "Old".into(), command: r"C:\Old\old.exe".into(), source: r"HKLM\Run".into(), all_users: true, disabled: true },
            ],
            &[r"c:\tools\tool.exe".into()],
        );
        assert_eq!(rows[0].owner, "Old");
        assert!(rows[0].disabled && rows[0].scope == "All users" && !rows[0].running);
        assert!(rows[1].running, "matched without regard to case");
        assert_eq!(rows[1].program.as_deref(), Some(r"C:\Tools\tool.exe"));
    }
}
