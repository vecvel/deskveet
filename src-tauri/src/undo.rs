//! The undo log: every action (including dry runs) with paths and timestamps, as one JSON file.

use crate::fsutil::iso;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_ENTRIES: usize = 5000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub run: String,
    pub ts: u64,
    pub at: String,
    pub rule: String,
    pub label: String,
    /// trash | tool | erase | dry-run
    pub action: String,
    /// ok | failed
    pub status: String,
    pub path: Option<String>,
    pub trash_path: Option<String>,
    pub bytes: u64,
    pub command: Option<Vec<String>>,
    pub error: Option<String>,
    #[serde(default)]
    pub restored: bool,
}

impl Entry {
    pub fn new(run: &str, ts: u64, seq: usize, rule: &str, label: &str, action: &str) -> Entry {
        Entry {
            id: format!("{run}-{seq}"),
            run: run.into(),
            ts,
            at: iso(ts),
            rule: rule.into(),
            label: label.into(),
            action: action.into(),
            status: "ok".into(),
            path: None,
            trash_path: None,
            bytes: 0,
            command: None,
            error: None,
            restored: false,
        }
    }
}

pub struct UndoLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl UndoLog {
    pub fn new(path: PathBuf) -> UndoLog {
        UndoLog { path, lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Vec<Entry> {
        fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write(&self, entries: &[Entry]) -> io::Result<()> {
        if let Some(d) = self.path.parent() {
            fs::create_dir_all(d)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
        fs::rename(tmp, &self.path)
    }

    pub fn append(&self, new: &[Entry]) -> io::Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all = self.read();
        all.extend_from_slice(new);
        if all.len() > MAX_ENTRIES {
            all.drain(..all.len() - MAX_ENTRIES);
        }
        self.write(&all)
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Entry> {
        let _g = self.lock.lock().unwrap();
        let mut v = self.read();
        v.reverse();
        v
    }

    pub fn get(&self, id: &str) -> Option<Entry> {
        self.list().into_iter().find(|e| e.id == id)
    }

    pub fn mark_restored(&self, id: &str) -> io::Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all = self.read();
        for e in all.iter_mut().filter(|e| e.id == id) {
            e.restored = true;
        }
        self.write(&all)
    }

    pub fn clear(&self) -> io::Result<()> {
        let _g = self.lock.lock().unwrap();
        self.write(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_list_restore_flag_and_cap() {
        let t = tempfile::tempdir().unwrap();
        let log = UndoLog::new(t.path().join("sub/undo.json"));
        assert!(log.list().is_empty());
        let mut e = Entry::new("r1", 1_700_000_000, 0, "rule.a", "A", "trash");
        e.path = Some("/x/a".into());
        log.append(&[e.clone(), Entry::new("r1", 1_700_000_001, 1, "rule.b", "B", "dry-run")]).unwrap();
        let l = log.list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].label, "B", "newest first");
        assert_eq!(l[1].at, "2023-11-14T22:13:20Z");
        log.mark_restored("r1-0").unwrap();
        assert!(log.get("r1-0").unwrap().restored);
        assert!(!log.get("r1-1").unwrap().restored);
        // corrupt file is treated as empty, not a crash
        fs::write(log.path(), b"not json").unwrap();
        assert!(log.list().is_empty());
        // cap
        let many: Vec<Entry> = (0..MAX_ENTRIES + 10).map(|i| Entry::new("r", 1, i, "x", "x", "trash")).collect();
        log.append(&many).unwrap();
        assert_eq!(log.list().len(), MAX_ENTRIES);
        log.clear().unwrap();
        assert!(log.list().is_empty());
    }
}
