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
