//! Apps on disk: what uninstalling an app removes, data left behind by apps that are gone, and the launch
//! items that start with the Mac. Everything here only lists; removing goes through the normal preview,
//! safety checks and Trash.

use crate::fsutil;
use crate::platform::{Bundle, LaunchDef};
use crate::safety::{Allowed, Safety};
use crate::sys::ProcInfo;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Folders in ~/Library where apps keep per-user data, and what to call each kind.
const LIBRARY_KINDS: &[(&str, &str)] = &[
    ("Application Support", "Support files"),
    ("Caches", "Caches"),
    ("HTTPStorages", "Web storage"),
    ("WebKit", "Web data"),
    ("Logs", "Logs"),
    ("Saved Application State", "Saved window state"),
    ("Preferences", "Settings"),
];

/// Why an installed app cannot be removed here (`None` when it can).
pub fn locked_reason(b: &Bundle) -> Option<&'static str> {
    let id = b.bundle_id.to_lowercase();
    if id.starts_with("com.apple.") || b.path.starts_with("/System") {
        Some("Part of macOS")
    } else if id == "app.deskveet.desktop" {
        Some("This app")
    } else {
        None
    }
}

/// A folder or file name that is a reverse-DNS app id: `com.vendor.product` (three parts or more).
pub fn looks_like_bundle_id(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 3
        && parts[0].len() >= 2
        && parts[0].len() <= 12
        && parts[0].chars().all(|c| c.is_ascii_lowercase())
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

/// The app id an entry in a Library folder belongs to, if its name is one.
fn entry_id(kind_dir: &str, name: &str) -> Option<String> {
    let base = match kind_dir {
        "Saved Application State" => name.strip_suffix(".savedState")?,
        "Preferences" => name.strip_suffix(".plist")?,
        "HTTPStorages" => name.strip_suffix(".binarycookies").unwrap_or(name),
        _ => name,
    };
    looks_like_bundle_id(base).then(|| base.to_string())
}

/// True when some app the system knows has this id, or an id it belongs to (`com.x.app` for
/// `com.x.app.helper`) or that belongs to it.
pub fn has_app(id: &str, known: &HashSet<String>) -> bool {
    let id = id.to_lowercase();
    known.contains(&id) || known.iter().any(|k| k.starts_with(&format!("{id}.")) || id.starts_with(&format!("{k}.")))
}

#[derive(Serialize, Clone, Debug)]
pub struct DataItem {
    pub path: String,
    /// "Caches", "Settings", ...
    pub kind: String,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct LeftoverGroup {
    /// The app id the data belongs to.
    pub id: String,
    pub total: u64,
    /// Newest change in any of its items (unix seconds).
    pub newest: i64,
    pub items: Vec<DataItem>,
}

fn measure_items(found: Vec<(PathBuf, &'static str)>) -> Vec<DataItem> {
    found
        .into_par_iter()
        .map(|(p, kind)| {
            let m = fsutil::measure(&p);
            DataItem { path: p.to_string_lossy().into_owned(), kind: kind.to_string(), size: m.bytes, mtime: m.mtime }
        })
        .collect()
}

/// Data in ~/Library named after an app id whose app is not on this Mac any more. Apple's own data, data
/// of any app or component the system still knows, data changed after `changed_before` (something is still
/// writing it, such as a command-line tool) and anything the safety rules protect are left out.
pub fn leftovers(home: &Path, known: &HashSet<String>, safety: &Safety, changed_before: i64) -> Vec<LeftoverGroup> {
    let lib = home.join("Library");
    let mut found = Vec::new();
    for (dir, kind) in LIBRARY_KINDS {
        let Ok(rd) = std::fs::read_dir(lib.join(dir)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = entry_id(dir, &name) else { continue };
            if id.to_lowercase().starts_with("com.apple.") || has_app(&id, known) {
                continue;
            }
            let p = e.path();
            if safety.check(&p, &Allowed::UserPick).is_ok() {
                found.push((id.to_lowercase(), p, *kind));
            }
        }
    }
    let ids: Vec<String> = found.iter().map(|(id, _, _)| id.clone()).collect();
    let items = measure_items(found.into_iter().map(|(_, p, k)| (p, k)).collect());
    let mut groups: BTreeMap<String, LeftoverGroup> = BTreeMap::new();
    for (id, it) in ids.into_iter().zip(items) {
        if it.size == 0 {
            continue;
        }
        let g = groups.entry(id.clone()).or_insert_with(|| LeftoverGroup { id, total: 0, newest: 0, items: Vec::new() });
        g.total += it.size;
        g.newest = g.newest.max(it.mtime);
        g.items.push(it);
    }
    let mut out: Vec<LeftoverGroup> = groups.into_values().filter(|g| g.newest <= changed_before).collect();
    out.sort_by(|a, b| b.total.cmp(&a.total));
    out
}

/// What uninstalling `b` removes: the app itself, then its data in ~/Library (by id and by name).
pub fn uninstall_items(home: &Path, b: &Bundle, name: &str, safety: &Safety) -> Vec<DataItem> {
    let lib = home.join("Library");
    let mut found: Vec<(PathBuf, &'static str)> = vec![(b.path.clone(), "The app")];
    for p in crate::apps::data_paths(home, &b.bundle_id, name) {
        let kind = LIBRARY_KINDS.iter().find(|(d, _)| p.starts_with(lib.join(d))).map_or("Data", |(_, k)| *k);
        found.push((p, kind));
    }
    let cookies = lib.join("HTTPStorages").join(format!("{}.binarycookies", b.bundle_id));
    if cookies.exists() {
        found.push((cookies, "Web storage"));
    }
    let mut seen = HashSet::new();
    found.retain(|(p, _)| seen.insert(p.clone()) && safety.check(p, &Allowed::UserPick).is_ok());
    measure_items(found)
}

#[derive(Serialize, Clone, Debug)]
pub struct StartupItem {
    pub label: String,
    pub plist: String,
    pub program: Option<String>,
    /// The app it belongs to, or its vendor from the label.
    pub owner: String,
    /// "Your account", "All users" or "System helper".
    pub scope: &'static str,
    pub running: bool,
    pub run_at_load: bool,
    pub keep_alive: bool,
    pub disabled: bool,
}

/// The app name in a path like `/Applications/Docker.app/Contents/...`.
fn app_in_path(p: &str) -> Option<String> {
    let i = p.find(".app/").or_else(|| p.ends_with(".app").then(|| p.len() - 4))?;
    let start = p[..i].rfind('/').map_or(0, |s| s + 1);
    Some(p[start..i].to_string())
}

/// Who a launch item belongs to: the app its program lives in, else the vendor in its label.
fn owner_of(d: &LaunchDef) -> String {
    if let Some(app) = d.program.as_deref().and_then(app_in_path) {
        return app;
    }
    let parts: Vec<&str> = d.label.split('.').collect();
    match parts.get(1).filter(|_| parts.len() >= 3) {
        Some(v) => {
            let mut c = v.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
        None => d.label.clone(),
    }
}

pub fn startup_items(home: &Path, defs: Vec<LaunchDef>, procs: &[ProcInfo]) -> Vec<StartupItem> {
    let mut out: Vec<StartupItem> = defs
        .into_iter()
        .map(|mut d| {
            // a file macOS cannot read the label of is still named by its file name (com.vendor.thing.plist)
            if d.label.is_empty() {
                d.label = d.plist.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            }
            let scope = if d.plist.starts_with(home) {
                "Your account"
            } else if d.plist.to_string_lossy().contains("/LaunchDaemons/") {
                "System helper"
            } else {
                "All users"
            };
            let running = d.program.as_deref().is_some_and(|prog| {
                let file = Path::new(prog).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                procs.iter().any(|p| p.cmd.starts_with(prog) || (!file.is_empty() && p.name == file))
            });
            StartupItem {
                owner: owner_of(&d),
                label: d.label,
                plist: d.plist.to_string_lossy().into_owned(),
                program: d.program,
                scope,
                running,
                run_at_load: d.run_at_load,
                keep_alive: d.keep_alive,
                disabled: d.disabled,
            }
        })
        .collect();
    out.sort_by(|a, b| a.owner.to_lowercase().cmp(&b.owner.to_lowercase()).then(a.label.cmp(&b.label)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn put(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![1u8; bytes]).unwrap();
    }

    #[test]
    fn app_ids_are_recognised_and_matched_to_apps() {
        assert!(looks_like_bundle_id("com.docker.docker") && looks_like_bundle_id("org.mozilla.firefox"));
        assert!(!looks_like_bundle_id("Google") && !looks_like_bundle_id("com.google") && !looks_like_bundle_id("Com.X.Y") && !looks_like_bundle_id("com..x"));
        let known: HashSet<String> = ["com.microsoft.vscode", "com.tinyspeck.slackmacgap.helper"].iter().map(|s| s.to_string()).collect();
        assert!(has_app("com.microsoft.VSCode", &known), "same id, any case");
        assert!(has_app("com.microsoft.VSCode.ShipIt", &known), "data of a part of an installed app");
        assert!(has_app("com.tinyspeck.slackmacgap", &known), "an app whose helper is installed");
        assert!(!has_app("com.microsoft.vscodeinsiders", &known), "a different app with a similar id");
    }

    /// Every file and folder below `d`, deepest first (so a folder is aged after what is inside it).
    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            }
            out.push(p);
        }
        out
    }

    #[test]
    fn leftovers_only_lists_data_of_apps_that_are_gone_and_never_apple_or_protected() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let lib = home.join("Library");
        put(&lib.join("Application Support/com.gone.app/db.sqlite"), 5_000);
        put(&lib.join("Caches/com.gone.app/cache.bin"), 7_000);
        put(&lib.join("Preferences/com.gone.app.plist"), 300);
        put(&lib.join("Saved Application State/com.gone.app.savedState/window.data"), 200);
        put(&lib.join("Caches/com.here.app/cache.bin"), 9_000);
        put(&lib.join("Caches/com.apple.Safari/cache.bin"), 9_000);
        put(&lib.join("Application Support/Google/Chrome/x"), 9_000);
        put(&lib.join("Caches/com.empty.app/.keep"), 0);
        let known: HashSet<String> = ["com.here.app".to_string()].into();
        put(&lib.join("Caches/org.busy.tool/c"), 1_000);
        let week_ago = crate::scan::now_secs() as i64 - 7 * 86_400;
        // everything but org.busy.tool was last changed long ago (folders too: adding a file changes them)
        for p in walk(&lib) {
            if !p.to_string_lossy().contains("org.busy.tool") {
                let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
                let old = [libc::timeval { tv_sec: 1_000_000, tv_usec: 0 }; 2];
                unsafe { libc::utimes(c.as_ptr(), old.as_ptr()) };
            }
        }
        let g = leftovers(&home, &known, &Safety::new(home.clone()), week_ago);
        assert_eq!(g.len(), 1, "{g:?}");
        assert_eq!(g[0].id, "com.gone.app");
        let mut kinds: Vec<&str> = g[0].items.iter().map(|i| i.kind.as_str()).collect();
        kinds.sort();
        assert_eq!(kinds, ["Caches", "Saved window state", "Settings", "Support files"]);
        assert!(g[0].total >= 12_500);
    }

    #[test]
    fn uninstall_lists_the_app_and_its_own_data_only() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let lib = home.join("Library");
        let app = home.join("Applications/Example.app");
        put(&app.join("Contents/MacOS/Example"), 4_000);
        put(&lib.join("Caches/com.example.app/c"), 1_000);
        put(&lib.join("Application Support/Example/s"), 1_000);
        put(&lib.join("Preferences/com.example.app.plist"), 100);
        put(&lib.join("HTTPStorages/com.example.app.binarycookies"), 100);
        put(&lib.join("Caches/com.other.app/c"), 1_000);
        let b = Bundle { path: app.clone(), bundle_id: "com.example.app".into(), version: "1.0".into() };
        let items = uninstall_items(&home, &b, "Example", &Safety::new(home.clone()));
        let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(items[0].kind, "The app");
        assert_eq!(items.len(), 5, "{paths:?}");
        assert!(paths.iter().all(|p| !p.contains("com.other.app")));
        assert!(locked_reason(&Bundle { path: "/Applications/Safari.app".into(), bundle_id: "com.apple.Safari".into(), version: String::new() }).is_some());
        assert!(locked_reason(&b).is_none());
    }

    #[test]
    fn startup_items_name_their_app_or_vendor_and_whether_they_run() {
        let home = PathBuf::from("/Users/me");
        let defs = vec![
            LaunchDef { plist: "/Library/LaunchDaemons/com.docker.socket.plist".into(), label: "com.docker.socket".into(), program: Some("/Applications/Docker.app/Contents/MacOS/com.docker.socket".into()), ..Default::default() },
            LaunchDef { plist: "/Library/LaunchAgents/com.microsoft.update.agent.plist".into(), label: "com.microsoft.update.agent".into(), program: Some("/Library/Application Support/Microsoft/MAU2.0/agent".into()), run_at_load: true, ..Default::default() },
            LaunchDef { plist: "/Users/me/Library/LaunchAgents/x.plist".into(), label: "x".into(), program: None, ..Default::default() },
            LaunchDef { plist: "/Users/me/Library/LaunchAgents/com.google.keystone.agent.plist".into(), ..Default::default() },
        ];
        let procs = vec![ProcInfo { pid: 1, name: "agent".into(), cmd: "/Library/Application Support/Microsoft/MAU2.0/agent --x".into() }];
        let s = startup_items(&home, defs, &procs);
        let docker = s.iter().find(|i| i.label == "com.docker.socket").unwrap();
        assert_eq!((docker.owner.as_str(), docker.scope, docker.running), ("Docker", "System helper", false));
        let ms = s.iter().find(|i| i.label.starts_with("com.microsoft")).unwrap();
        assert_eq!((ms.owner.as_str(), ms.scope, ms.running), ("Microsoft", "All users", true));
        assert_eq!(s.iter().find(|i| i.label == "x").unwrap().scope, "Your account");
        let unreadable = s.iter().find(|i| i.label == "com.google.keystone.agent").expect("named after its file");
        assert_eq!(unreadable.owner, "Google");
    }
}
