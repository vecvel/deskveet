//! Live sizes: FSEvents on each enabled rule's folders marks the rule "dirty"; a slow loop re-measures dirty
//! rules (every few seconds while the window is visible, at most every 10 minutes in the background).

use crate::cleaner::Engine;
use notify::{RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct LiveWatch {
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    roots: Mutex<Vec<(PathBuf, Vec<String>)>>,
    dirty: Mutex<HashSet<String>>,
    measured: Mutex<HashMap<String, Instant>>,
    /// Our own folders: writes there never mark a rule dirty.
    ignore: Mutex<Vec<PathBuf>>,
}

impl LiveWatch {
    /// (Re)watch the folders of every enabled rule.
    pub fn rebuild(self: &Arc<Self>, engine: &Engine, disabled: &HashSet<String>, ignore: Vec<PathBuf>) {
        let reg = engine.registry();
        let mut by_root: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for r in reg.rules.iter().filter(|r| !disabled.contains(&r.id)) {
            for root in engine.watch_roots(r) {
                by_root.entry(root).or_default().push(r.id.clone());
            }
        }
        // Watch the outermost folders only; nested roots are covered by their parent's recursive watch.
        let mut tops: Vec<PathBuf> = by_root.keys().cloned().collect();
        tops.sort();
        tops.dedup();
        let tops: Vec<PathBuf> = tops.iter().filter(|p| !tops.iter().any(|q| q != *p && p.starts_with(q))).cloned().collect();

        let me = Arc::downgrade(self);
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let (Ok(ev), Some(me)) = (res, me.upgrade()) {
                me.on_paths(&ev.paths);
            }
        });
        let Ok(mut w) = watcher else { return };
        for t in &tops {
            let _ = w.watch(t, RecursiveMode::Recursive);
        }
        *self.ignore.lock().unwrap() = ignore;
        *self.roots.lock().unwrap() = by_root.into_iter().collect();
        *self.watcher.lock().unwrap() = Some(w);
    }

    pub fn watched_rules(&self) -> usize {
        self.roots.lock().unwrap().iter().flat_map(|(_, ids)| ids).collect::<HashSet<_>>().len()
    }

    fn on_paths(&self, paths: &[PathBuf]) {
        let ignore = self.ignore.lock().unwrap();
        let roots = self.roots.lock().unwrap();
        let mut dirty = self.dirty.lock().unwrap();
        for p in paths.iter().filter(|p| !ignore.iter().any(|i| p.starts_with(i))) {
            for (root, ids) in roots.iter() {
                if p.starts_with(root) {
                    dirty.extend(ids.iter().cloned());
                }
            }
        }
    }

    /// Mark a rule as needing a fresh measurement (used by tests and after a clean).
    pub fn touch(&self, path: &Path) {
        self.on_paths(&[path.to_path_buf()]);
    }

    /// Dirty rules not measured within `min_gap`; they are taken out of the dirty set.
    pub fn take_due(&self, min_gap: Duration) -> Vec<String> {
        let mut dirty = self.dirty.lock().unwrap();
        let mut measured = self.measured.lock().unwrap();
        let due: Vec<String> = dirty.iter().filter(|id| measured.get(*id).is_none_or(|t| t.elapsed() >= min_gap)).cloned().collect();
        for id in &due {
            dirty.remove(id);
            measured.insert(id.clone(), Instant::now());
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_mark_rules_dirty_and_are_rate_limited() {
        let w = Arc::new(LiveWatch::default());
        *w.roots.lock().unwrap() = vec![(PathBuf::from("/h/Library/Caches"), vec!["appcache".into()]), (PathBuf::from("/h/dd"), vec!["xcode".into()])];
        *w.ignore.lock().unwrap() = vec![PathBuf::from("/h/Library/Caches/app.deskveet.desktop")];
        w.touch(Path::new("/h/dd/A-1/x"));
        w.touch(Path::new("/h/Library/Caches/app.deskveet.desktop/scan.json")); // our own write: ignored
        let mut due = w.take_due(Duration::from_secs(600));
        due.sort();
        assert_eq!(due, vec!["xcode".to_string()]);
        w.touch(Path::new("/h/dd/A-1/y"));
        assert!(w.take_due(Duration::from_secs(600)).is_empty(), "measured a moment ago: waits");
        assert_eq!(w.take_due(Duration::ZERO), vec!["xcode".to_string()], "still dirty, due once the gap passes");
    }
}
