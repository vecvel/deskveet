//! The Apps tab: running applications with their memory, CPU and storage, and quick Quit / Force Quit.

use crate::fsutil;
use crate::platform::RunningApp;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Apps the system needs; Deskveet never offers to quit them.
const PROTECTED: &[(&str, &str)] = &[
    ("com.apple.finder", "Finder relaunches itself; use the Apple menu to relaunch it"),
    ("com.apple.dock", "part of macOS"),
    ("com.apple.loginwindow", "part of macOS"),
    ("com.apple.systemuiserver", "part of macOS"),
    ("com.apple.controlcenter", "part of macOS"),
    ("com.apple.notificationcenterui", "part of macOS"),
    ("app.deskveet.desktop", "this app"),
];

pub fn protected_reason(app: &RunningApp, me: u32) -> Option<&'static str> {
    if app.pid == me {
        return Some("this app");
    }
    PROTECTED.iter().find(|(id, _)| *id == app.bundle_id).map(|(_, why)| *why)
}

#[derive(Serialize, Clone, Debug)]
pub struct AppRow {
    pub pid: u32,
    pub name: String,
    pub bundle_id: String,
    pub path: Option<String>,
    /// Resident memory of the app and every helper process inside its bundle.
    pub mem: u64,
    pub cpu: f32,
    pub procs: u32,
    /// The .app bundle on disk; `None` while it is still being measured.
    pub bundle_bytes: Option<u64>,
    /// Caches, support files, web data and saved state in ~/Library; `None` while measuring.
    pub data_bytes: Option<u64>,
    pub protected: Option<&'static str>,
    /// data: URL of a 64px PNG icon.
    pub icon: Option<String>,
}

/// Where an app keeps its per-user data (outside sandbox containers, which macOS guards per app).
pub fn data_paths(home: &Path, bundle_id: &str, name: &str) -> Vec<PathBuf> {
    let lib = home.join("Library");
    let mut v = Vec::new();
    for key in [bundle_id, name].iter().filter(|k| !k.is_empty() && !k.contains('/') && **k != "..") {
        for dir in ["Application Support", "Caches", "HTTPStorages", "WebKit", "Logs"] {
            v.push(lib.join(dir).join(key));
        }
    }
    if !bundle_id.is_empty() {
        v.push(lib.join("Saved Application State").join(format!("{bundle_id}.savedState")));
        v.push(lib.join("Preferences").join(format!("{bundle_id}.plist")));
    }
    let mut seen = HashSet::new();
    v.retain(|p| p.exists() && seen.insert(p.clone()));
    v
}

#[derive(Clone, Copy)]
struct Sizes {
    bundle: u64,
    data: u64,
    at: Instant,
}

/// Storage is slow to measure, so it is done in the background and cached for a while.
#[derive(Default)]
pub struct SizeCache {
    done: Mutex<HashMap<String, Sizes>>,
    pending: Mutex<HashSet<String>>,
}

const SIZE_TTL: Duration = Duration::from_secs(15 * 60);

impl SizeCache {
    pub fn get(&self, bundle_id: &str) -> Option<(u64, u64)> {
        self.done.lock().unwrap().get(bundle_id).map(|s| (s.bundle, s.data))
    }

    /// Measure any app that has no fresh size yet, on one low-impact background thread.
    pub fn request(self: &Arc<Self>, apps: &[RunningApp], home: &Path) {
        let todo: Vec<RunningApp> = {
            let done = self.done.lock().unwrap();
            let mut pending = self.pending.lock().unwrap();
            apps.iter()
                .filter(|a| !a.bundle_id.is_empty() && done.get(&a.bundle_id).is_none_or(|s| s.at.elapsed() > SIZE_TTL))
                .filter(|a| pending.insert(a.bundle_id.clone()))
                .cloned()
                .collect()
        };
        if todo.is_empty() {
            return;
        }
        let (me, home) = (self.clone(), home.to_path_buf());
        std::thread::spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().ok();
            for a in todo {
                let run = || {
                    let bundle = a.bundle_path.as_deref().map(|p| fsutil::measure(p).bytes).unwrap_or(0);
                    let data = data_paths(&home, &a.bundle_id, &a.name).iter().map(|p| fsutil::measure(p).bytes).sum();
                    (bundle, data)
                };
                let (bundle, data) = match &pool { Some(p) => p.install(run), None => run() };
                me.done.lock().unwrap().insert(a.bundle_id.clone(), Sizes { bundle, data, at: Instant::now() });
                me.pending.lock().unwrap().remove(&a.bundle_id);
            }
        });
    }
}

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn system_apps_and_self_are_protected() {
        let app = |id: &str, pid| RunningApp { pid, name: "x".into(), bundle_id: id.into(), bundle_path: None };
        assert!(protected_reason(&app("com.apple.finder", 10), 1).is_some());
        assert!(protected_reason(&app("com.apple.dock", 10), 1).is_some());
        assert!(protected_reason(&app("com.example.editor", 1), 1).is_some(), "own pid");
        assert!(protected_reason(&app("com.example.editor", 10), 1).is_none());
    }

    #[test]
    fn data_paths_only_lists_existing_folders_and_skips_containers() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path();
        for d in ["Library/Caches/com.example.app", "Library/Application Support/Example", "Library/Containers/com.example.app"] {
            std::fs::create_dir_all(h.join(d)).unwrap();
        }
        let p = data_paths(h, "com.example.app", "Example");
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p.iter().all(|x| !x.to_string_lossy().contains("Containers")));
        assert!(data_paths(h, "../evil", "..").is_empty());
    }
}
