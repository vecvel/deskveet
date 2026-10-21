use super::{MemExtra, Platform, Pressure, ProcFacts, ProcessInfo};
use std::ffi::{c_void, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Default)]
pub struct Mac;

extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
}

extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

pub fn background_thread() {
    const QOS_CLASS_UTILITY: u32 = 0x11;
    // SAFETY: affects only the calling thread.
    unsafe {
        pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
    }
}

pub fn trim_memory() {
    // SAFETY: a null zone means "all zones"; goal 0 means "as much as possible".
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

// ───────────────────────────── sysctl / mach ─────────────────────────────

fn sysctl_raw<T: Copy + Default>(name: &str) -> Option<T> {
    let cname = CString::new(name).ok()?;
    let mut out = T::default();
    let mut len = std::mem::size_of::<T>();
    // SAFETY: `out` is a valid, writable T of `len` bytes; the name is NUL-terminated.
    let rc = unsafe {
        libc::sysctlbyname(cname.as_ptr(), &mut out as *mut T as *mut c_void, &mut len, std::ptr::null_mut(), 0)
    };
    (rc == 0).then_some(out)
}

/// `struct xsw_usage` from <sys/sysctl.h>.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XswUsage {
    total: u64,
    avail: u64,
    used: u64,
    pagesize: u32,
    encrypted: u8,
}

/// `vm_statistics64` from <mach/vm_statistics.h> (38 ints).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VmStat64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

const HOST_VM_INFO64: i32 = 4;

extern "C" {
    fn mach_host_self() -> u32;
    fn host_statistics64(host: u32, flavor: i32, out: *mut i32, count: *mut u32) -> i32;
}

fn vm_stats() -> Option<VmStat64> {
    let mut s = VmStat64::default();
    let mut count = (std::mem::size_of::<VmStat64>() / 4) as u32;
    // SAFETY: `s` is a properly sized vm_statistics64 and `count` its size in ints.
    let kr = unsafe { host_statistics64(mach_host_self(), HOST_VM_INFO64, &mut s as *mut _ as *mut i32, &mut count) };
    (kr == 0).then_some(s)
}

/// Fallback when the mach call fails: parse `vm_stat`.
fn vm_stats_from_cli() -> Option<VmStat64> {
    let out = Command::new("/usr/bin/vm_stat").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let get = |key: &str| -> u32 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    };
    Some(VmStat64 {
        wire_count: get("Pages wired down"),
        purgeable_count: get("Pages purgeable"),
        compressor_page_count: get("Pages occupied by compressor"),
        // vm_stat reports "File-backed pages" (external) and "Anonymous pages" (internal).
        external_page_count: get("File-backed pages"),
        internal_page_count: get("Anonymous pages"),
        ..Default::default()
    })
}

// ───────────────────────────── CoreFoundation (purgeable space) ─────────────────────────────

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFURLVolumeAvailableCapacityForImportantUsageKey: *const c_void;
    fn CFURLCreateFromFileSystemRepresentation(alloc: *const c_void, buf: *const u8, len: isize, is_dir: u8) -> *const c_void;
    fn CFURLCopyResourcePropertyForKey(url: *const c_void, key: *const c_void, value: *mut *const c_void, err: *mut *const c_void) -> u8;
    fn CFNumberGetValue(n: *const c_void, ty: isize, out: *mut c_void) -> u8;
    fn CFRelease(o: *const c_void);
}

fn cf_available_important(mount: &Path) -> Option<u64> {
    let bytes = mount.as_os_str().as_bytes();
    // SAFETY: standard CF create/copy/release pairing; all pointers are checked for null.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 1);
        if url.is_null() {
            return None;
        }
        let mut value: *const c_void = std::ptr::null();
        let mut err: *const c_void = std::ptr::null();
        let ok = CFURLCopyResourcePropertyForKey(url, kCFURLVolumeAvailableCapacityForImportantUsageKey, &mut value, &mut err);
        let mut result = None;
        if ok != 0 && !value.is_null() {
            let mut n: i64 = 0;
            if CFNumberGetValue(value, 4 /* kCFNumberSInt64Type */, &mut n as *mut i64 as *mut c_void) != 0 && n >= 0 {
                result = Some(n as u64);
            }
            CFRelease(value);
        }
        if !err.is_null() {
            CFRelease(err);
        }
        CFRelease(url);
        result
    }
}

/// Spawn and reap in the background so finished helpers (qlmanage, open) never linger as zombies.
fn spawn_reaped(cmd: &mut Command) -> io::Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ───────────────────────────── Platform ─────────────────────────────

impl Platform for Mac {
    fn memory_extra(&self) -> Option<MemExtra> {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let vm = vm_stats().or_else(vm_stats_from_cli)?;
        let purgeable = vm.purgeable_count as u64;
        // Same definitions Activity Monitor uses.
        let app = (vm.internal_page_count as u64).saturating_sub(purgeable) * page;
        let wired = vm.wire_count as u64 * page;
        let compressed = vm.compressor_page_count as u64 * page;
        let cached = (vm.external_page_count as u64 + purgeable) * page;
        let pressure = match sysctl_raw::<i32>("kern.memorystatus_vm_pressure_level") {
            Some(4) => Pressure::Critical,
            Some(2) => Pressure::Warn,
            _ => Pressure::Normal,
        };
        let swap = sysctl_raw::<XswUsage>("vm.swapusage").unwrap_or_default();
        Some(MemExtra {
            app,
            wired,
            compressed,
            cached,
            used: app + wired + compressed,
            pressure,
            swap_used: swap.used,
            swap_total: swap.total,
        })
    }

    fn available_for_important_usage(&self, mount: &Path) -> Option<u64> {
        cf_available_important(mount)
    }

    fn is_user_volume(&self, mount: &Path, _fs: &str) -> bool {
        let m = mount.to_string_lossy();
        // On APFS the user-visible disk is the Data volume; "/" is the read-only sealed system snapshot.
        m == "/System/Volumes/Data" || (m.starts_with("/Volumes/") && !m.starts_with("/Volumes/com.apple.") && !m.contains("/Recovery"))
    }

    fn volume_label(&self, mount: &Path, fallback: &str) -> String {
        if mount == Path::new("/System/Volumes/Data") {
            // The boot volume is the entry in /Volumes that is a symlink to "/".
            if let Ok(rd) = std::fs::read_dir("/Volumes") {
                for e in rd.flatten() {
                    if e.file_type().is_ok_and(|t| t.is_symlink()) && std::fs::canonicalize(e.path()).is_ok_and(|c| c == Path::new("/")) {
                        return e.file_name().to_string_lossy().into_owned();
                    }
                }
            }
            return "Macintosh HD".into();
        }
        mount.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.to_string())
    }

    fn reveal(&self, path: &Path) -> io::Result<()> {
        spawn_reaped(Command::new("/usr/bin/open").arg("-R").arg(path))
    }

    fn open(&self, path: &Path) -> io::Result<()> {
        spawn_reaped(Command::new("/usr/bin/open").arg(path))
    }

    fn quick_look(&self, path: &Path) -> io::Result<()> {
        spawn_reaped(Command::new("/usr/bin/qlmanage").arg("-p").arg(path))
    }

    fn open_full_disk_access_settings(&self) -> io::Result<()> {
        spawn_reaped(Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"))
    }

    fn full_disk_access(&self) -> Option<bool> {
        // Folders macOS privacy protection guards. Listing one succeeds only with Full Disk Access.
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let mut blocked = false;
        for rel in ["Library/Safari", "Library/Mail", "Library/Messages", ".Trash", "Library/Application Support/com.apple.TCC"] {
            match std::fs::read_dir(home.join(rel)) {
                Ok(_) => return Some(true),
                Err(e) if e.raw_os_error() == Some(1) || e.kind() == io::ErrorKind::PermissionDenied => blocked = true,
                Err(_) => {}
            }
        }
        blocked.then_some(false)
    }

    fn pick_folder(&self) -> Option<PathBuf> {
        let out = Command::new("/usr/bin/osascript")
            .args(["-e", "POSIX path of (choose folder with prompt \"Choose a folder to scan\")"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then(|| PathBuf::from(s))
    }
}

// ───────────────────────────── ProcessInfo ─────────────────────────────

/// Never offered for quit, whatever the owner.
const PROTECTED_NAMES: &[&str] = &[
    "kernel_task", "launchd", "WindowServer", "loginwindow", "Finder", "Dock", "SystemUIServer", "ControlCenter",
    "NotificationCenter", "coreaudiod", "coreservicesd", "cfprefsd", "distnoted", "mds", "mds_stores", "mdworker_shared",
    "securityd", "trustd", "opendirectoryd", "powerd", "configd", "diskarbitrationd", "fseventsd", "hidd", "launchservicesd",
    "logd", "syslogd", "UserEventAgent", "universalaccessd", "WindowManager", "Spotlight", "sharingd", "airportd", "bluetoothd",
    "tccd", "amfid", "endpointsecurityd", "usermanagerd", "runningboardd", "watchdogd", "lsd", "secd", "pboard", "backupd",
    "Deskveet", "deskveet",
];

const SYSTEM_PREFIXES: &[&str] = &["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/", "/usr/bin/", "/bin/", "/Library/Apple/", "/private/var/db/"];

impl ProcessInfo for Mac {
    fn protected_reason(&self, p: &ProcFacts, me_uid: u32, me_pid: u32) -> Option<&'static str> {
        if p.pid <= 1 {
            return Some("system process");
        }
        if p.pid == me_pid {
            return Some("this app");
        }
        if p.uid == 0 {
            return Some("runs as root");
        }
        if p.uid != me_uid {
            return Some("owned by another user");
        }
        if PROTECTED_NAMES.contains(&p.name) {
            return Some("protected system process");
        }
        if let Some(exe) = p.exe {
            let s = exe.to_string_lossy();
            if SYSTEM_PREFIXES.iter().any(|pre| s.starts_with(pre)) {
                return Some("part of macOS");
            }
        }
        None
    }

    fn reveal_target(&self, exe: &Path) -> PathBuf {
        // Reveal the outermost .app bundle (Chrome helpers live deep inside the main app bundle).
        let mut found: Option<PathBuf> = None;
        let mut cur = PathBuf::new();
        for c in exe.components() {
            cur.push(c);
            if cur.extension().is_some_and(|e| e == "app") {
                found = Some(cur.clone());
                break;
            }
        }
        found.unwrap_or_else(|| exe.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(pid: u32, uid: u32, name: &'a str, exe: Option<&'a Path>) -> ProcFacts<'a> {
        ProcFacts { pid, uid, name, exe }
    }

    #[test]
    fn protects_system_and_root() {
        let m = Mac;
        assert!(m.protected_reason(&facts(1, 0, "launchd", None), 501, 999).is_some());
        assert!(m.protected_reason(&facts(120, 0, "something", None), 501, 999).is_some());
        assert!(m.protected_reason(&facts(300, 501, "WindowServer", None), 501, 999).is_some());
        assert!(m.protected_reason(&facts(300, 501, "kernel_task", None), 501, 999).is_some());
        assert!(m.protected_reason(&facts(999, 501, "deskveet", None), 501, 999).is_some());
        assert!(m.protected_reason(&facts(400, 502, "Notes", None), 501, 999).is_some());
        let sys = Path::new("/System/Library/CoreServices/Foo.app/Contents/MacOS/Foo");
        assert!(m.protected_reason(&facts(400, 501, "Foo", Some(sys)), 501, 999).is_some());
    }

    #[test]
    fn allows_normal_user_apps() {
        let m = Mac;
        let exe = Path::new("/Applications/Notes2.app/Contents/MacOS/Notes2");
        assert!(m.protected_reason(&facts(400, 501, "Notes2", Some(exe)), 501, 999).is_none());
    }

    #[test]
    fn reveal_target_picks_outer_bundle() {
        let m = Mac;
        let exe = Path::new("/Applications/Google Chrome.app/Contents/Frameworks/X.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Helper");
        assert_eq!(m.reveal_target(exe), PathBuf::from("/Applications/Google Chrome.app"));
        let plain = Path::new("/opt/homebrew/bin/node");
        assert_eq!(m.reveal_target(plain), PathBuf::from("/opt/homebrew/bin/node"));
    }

    #[test]
    fn full_disk_access_probe_gives_an_answer() {
        // Read-only: lists a protected folder. A plain test runner has no Full Disk Access.
        let r = Mac.full_disk_access();
        assert!(r.is_some(), "the probe should be able to tell on macOS");
        eprintln!("full disk access for this test binary: {r:?}");
    }

    #[test]
    fn vm_struct_matches_kernel_size() {
        assert_eq!(std::mem::size_of::<VmStat64>(), 152);
    }

    #[test]
    fn memory_numbers_are_sane() {
        let m = Mac.memory_extra().expect("memory");
        assert!(m.wired > 0 && m.used > m.wired);
    }
}
