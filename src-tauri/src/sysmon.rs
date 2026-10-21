//! Live system numbers. One long-lived `System` is reused; only what the UI shows is refreshed.

use crate::platform::{self, Current, Platform, Pressure, ProcFacts, ProcessInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::CStr;
use std::time::{Duration, Instant};
use sysinfo::{Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const DISK_TTL: Duration = Duration::from_secs(10);
const PROC_TTL: Duration = Duration::from_millis(500);

#[derive(Serialize, Clone)]
pub struct DiskRow {
    pub name: String,
    pub mount: String,
    pub fs: String,
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub purgeable: u64,
    pub removable: bool,
    pub used_pct: f32,
}

#[derive(Serialize)]
pub struct Mem {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
    pub cached: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub pressure: Pressure,
}

#[derive(Serialize)]
pub struct Cpu {
    pub overall: f32,
    pub cores: Vec<f32>,
    pub load: [f64; 3],
}

#[derive(Serialize, Clone)]
pub struct ProcRow {
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub mem: u64,
    /// Percent of one core (can exceed 100), like Activity Monitor.
    pub cpu: f32,
    pub path: Option<String>,
    pub reveal: Option<String>,
    pub start: u64,
    pub protected: Option<&'static str>,
}

#[derive(Serialize)]
pub struct Overview {
    pub host: String,
    pub home: String,
    pub uptime: u64,
    pub cpu: Cpu,
    pub mem: Mem,
    pub disks: Vec<DiskRow>,
    pub top_mem: Vec<ProcRow>,
    pub top_cpu: Vec<ProcRow>,
}

#[derive(Deserialize)]
pub struct ProcQuery {
    pub sort: String,
    pub limit: usize,
    #[serde(default)]
    pub query: String,
}

#[derive(Serialize)]
pub struct ProcList {
    pub rows: Vec<ProcRow>,
    pub total: usize,
}

pub struct Monitor {
    sys: System,
    disks: Disks,
    disk_cache: Option<(Instant, Vec<DiskRow>)>,
    proc_at: Option<Instant>,
    users: HashMap<u32, String>,
    plat: Current,
    me_uid: u32,
    me_pid: u32,
}

fn proc_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_user(UpdateKind::OnlyIfNotSet)
}

fn user_name(uid: u32) -> String {
    let mut buf = [0 as libc::c_char; 512];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: buffers are valid for the given sizes; `res` is only read when the call succeeded.
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut res) };
    if rc == 0 && !res.is_null() {
        unsafe { CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned()
    } else {
        uid.to_string()
    }
}

/// (total, available-to-unprivileged) bytes of the volume holding `path`.
fn statvfs(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is NUL-terminated and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let fr = st.f_frsize as u64;
    Some((st.f_blocks as u64 * fr, st.f_bavail as u64 * fr))
}

impl Monitor {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self {
            sys,
            disks: Disks::new(),
            disk_cache: None,
            proc_at: None,
            users: HashMap::new(),
            plat: platform::current(),
            me_uid: unsafe { libc::getuid() },
            me_pid: std::process::id(),
        }
    }

    /// Forget all cached state (process table, disks, user names) and return heap pages to the OS.
    pub fn release(&mut self) {
        self.sys = System::new();
        self.disks = Disks::new();
        self.disk_cache = None;
        self.proc_at = None;
        self.users = HashMap::new();
        platform::trim_memory();
    }

    pub fn plat(&self) -> &Current {
        &self.plat
    }

    fn refresh_procs(&mut self) {
        if self.proc_at.is_some_and(|t| t.elapsed() < PROC_TTL) {
            return;
        }
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind());
        self.proc_at = Some(Instant::now());
        platform::trim_memory();
    }

    fn disk_rows(&mut self) -> Vec<DiskRow> {
        if let Some((at, rows)) = &self.disk_cache {
            if at.elapsed() < DISK_TTL {
                return rows.clone();
            }
        }
        self.disks.refresh_specifics(true, sysinfo::DiskRefreshKind::nothing().with_storage());
        if self.disks.list().is_empty() {
            self.disks = Disks::new_with_refreshed_list_specifics(sysinfo::DiskRefreshKind::nothing().with_storage());
        }
        let mut rows: Vec<DiskRow> = Vec::new();
        for d in self.disks.list() {
            let mount = d.mount_point();
            let fs = d.file_system().to_string_lossy().into_owned();
            if !self.plat.is_user_volume(mount, &fs) || d.total_space() == 0 {
                continue;
            }
            // statvfs gives the raw free space; sysinfo's figure already folds purgeable space in on macOS.
            let (total, bavail) = statvfs(mount).unwrap_or((d.total_space(), d.available_space()));
            let bavail = bavail.min(total);
            let important = self.plat.available_for_important_usage(mount).unwrap_or(bavail).clamp(bavail, total);
            let used = total - important;
            rows.push(DiskRow {
                name: self.plat.volume_label(mount, &d.name().to_string_lossy()),
                mount: mount.to_string_lossy().into_owned(),
                fs,
                total,
                used,
                free: bavail,
                purgeable: important - bavail,
                removable: d.is_removable(),
                used_pct: used as f32 / total as f32 * 100.0,
            });
        }
        rows.sort_by(|a, b| a.removable.cmp(&b.removable).then(a.mount.cmp(&b.mount)));
        self.disk_cache = Some((Instant::now(), rows.clone()));
        rows
    }

    fn row(&mut self, pid: &Pid) -> Option<ProcRow> {
        let p = self.sys.process(*pid)?;
        let uid = p.user_id().map(|u| **u).unwrap_or(u32::MAX);
        let exe = p.exe();
        let name = exe
            .and_then(|e| e.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.name().to_string_lossy().into_owned());
        let protected = self.plat.protected_reason(&ProcFacts { pid: pid.as_u32(), uid, name: &name, exe }, self.me_uid, self.me_pid);
        let reveal = exe.map(|e| self.plat.reveal_target(e).to_string_lossy().into_owned());
        let row = ProcRow {
            pid: pid.as_u32(),
            user: self.users.entry(uid).or_insert_with(|| user_name(uid)).clone(),
            mem: p.memory(),
            cpu: p.cpu_usage(),
            path: exe.map(|e| e.to_string_lossy().into_owned()),
            reveal,
            start: p.start_time(),
            protected,
            name,
        };
        Some(row)
    }

    fn top(&mut self, by_mem: bool, n: usize, query: &str) -> (Vec<ProcRow>, usize) {
        let q = query.trim().to_lowercase();
        let mut keys: Vec<(Pid, u64, f32)> = self
            .sys
            .processes()
            .iter()
            .filter(|(pid, p)| {
                q.is_empty()
                    || pid.as_u32().to_string() == q
                    || p.name().to_string_lossy().to_lowercase().contains(&q)
                    || p.exe().is_some_and(|e| e.to_string_lossy().to_lowercase().contains(&q))
            })
            .map(|(pid, p)| (*pid, p.memory(), p.cpu_usage()))
            .collect();
        let total = keys.len();
        if by_mem {
            keys.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        } else {
            keys.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        }
        keys.truncate(n);
        let rows = keys.iter().filter_map(|(pid, _, _)| self.row(pid)).collect();
        (rows, total)
    }

    pub fn overview(&mut self) -> Overview {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.refresh_procs();

        let total = self.sys.total_memory();
        let extra = self.plat.memory_extra();
        let used = extra.map(|e| e.used).unwrap_or_else(|| self.sys.used_memory());
        let mem = Mem {
            total,
            used: used.min(total),
            free: total.saturating_sub(used),
            app: extra.map(|e| e.app).unwrap_or(0),
            wired: extra.map(|e| e.wired).unwrap_or(0),
            compressed: extra.map(|e| e.compressed).unwrap_or(0),
            cached: extra.map(|e| e.cached).unwrap_or(0),
            swap_used: extra.map(|e| e.swap_used).unwrap_or_else(|| self.sys.used_swap()),
            swap_total: extra.map(|e| e.swap_total).unwrap_or_else(|| self.sys.total_swap()),
            pressure: extra.map(|e| e.pressure).unwrap_or_default(),
        };
        let la = System::load_average();
        let cpu = Cpu {
            overall: self.sys.global_cpu_usage(),
            cores: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            load: [la.one, la.five, la.fifteen],
        };
        let disks = self.disk_rows();
        let top_mem = self.top(true, 5, "").0;
        let top_cpu = self.top(false, 5, "").0;
        Overview {
            host: System::host_name().unwrap_or_default(),
            home: std::env::var("HOME").unwrap_or_default(),
            uptime: System::uptime(),
            cpu,
            mem,
            disks,
            top_mem,
            top_cpu,
        }
    }

    pub fn processes(&mut self, q: &ProcQuery) -> ProcList {
        self.sys.refresh_cpu_usage();
        self.refresh_procs();
        let (rows, total) = self.top(q.sort == "mem", q.limit.clamp(1, 500), &q.query);
        ProcList { rows, total }
    }

    /// Quit (SIGTERM) or force quit (SIGKILL). Re-validates everything: the UI is not trusted.
    pub fn signal(&mut self, pid: u32, start: u64, force: bool) -> Result<(), String> {
        let p = Pid::from_u32(pid);
        self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, proc_kind());
        let row = self.row(&p).ok_or("That process is no longer running.")?;
        if row.start != start {
            return Err("The process id was reused by another program. Refresh and try again.".into());
        }
        if let Some(why) = row.protected {
            return Err(format!("{} is protected ({why}).", row.name));
        }
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: plain kill(2) on a pid we just validated as ours and unprotected.
        if unsafe { libc::kill(pid as libc::pid_t, sig) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        self.proc_at = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_has_a_disk_and_memory() {
        let mut m = Monitor::new();
        let o = m.overview();
        assert!(o.mem.total > 0);
        assert!(!o.disks.is_empty());
        for d in &o.disks {
            assert_eq!(d.used + d.purgeable + d.free, d.total, "{}", d.mount);
        }
        // CoreFoundation must report >= raw free space for the data volume.
        let mac = platform::current();
        let d = &o.disks[0];
        let important = mac.available_for_important_usage(std::path::Path::new(&d.mount)).expect("CF capacity");
        assert!(important >= d.free);
        assert!(!o.top_mem.is_empty());
    }

    #[test]
    fn own_process_is_protected_and_refuses_signal() {
        let mut m = Monitor::new();
        let me = m.me_pid;
        let l = m.processes(&ProcQuery { sort: "mem".into(), limit: 500, query: me.to_string() });
        let row = l.rows.iter().find(|r| r.pid == me).expect("self visible");
        assert!(row.protected.is_some());
        assert!(m.signal(me, row.start, true).is_err());
    }

    #[test]
    fn pid_one_cannot_be_signalled() {
        let mut m = Monitor::new();
        assert!(m.signal(1, 0, true).is_err());
    }
}
