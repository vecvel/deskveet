//! Minimal fallback so the crate still compiles on Windows/Linux. Real implementations come later.
use super::{MemExtra, Platform, ProcFacts, ProcessInfo};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Generic;

impl Platform for Generic {
    fn memory_extra(&self) -> Option<MemExtra> {
        None
    }
    fn available_for_important_usage(&self, _: &Path) -> Option<u64> {
        None
    }
    fn is_user_volume(&self, _: &Path, _: &str) -> bool {
        true
    }
    fn volume_label(&self, mount: &Path, fallback: &str) -> String {
        mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| fallback.to_string())
    }
    fn reveal(&self, path: &Path) -> io::Result<()> {
        self.open(path.parent().unwrap_or(path))
    }
    fn open(&self, path: &Path) -> io::Result<()> {
        let prog = if cfg!(windows) { "explorer" } else { "xdg-open" };
        std::process::Command::new(prog).arg(path).spawn().map(|_| ())
    }
    fn quick_look(&self, path: &Path) -> io::Result<()> {
        self.open(path)
    }
    fn open_full_disk_access_settings(&self) -> io::Result<()> {
        Ok(())
    }
    fn pick_folder(&self) -> Option<PathBuf> {
        None
    }
}

impl ProcessInfo for Generic {
    fn protected_reason(&self, p: &ProcFacts, me_uid: u32, me_pid: u32) -> Option<&'static str> {
        if p.pid <= 1 || p.pid == me_pid {
            return Some("protected");
        }
        (p.uid != me_uid).then_some("owned by another user")
    }
    fn reveal_target(&self, exe: &Path) -> PathBuf {
        exe.to_path_buf()
    }
}
