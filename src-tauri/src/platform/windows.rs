//! Windows: memory, drives, File Explorer, notifications, dialogs and process protection.

use super::{Apps, Disk, MemExtra, Platform, Power, Pressure, ProcFacts, ProcessInfo, RunningApp};
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The app's identity for notifications; the installer gives the Start menu shortcut the same id.
pub const APP_ID: &str = "app.deskveet.desktop";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
pub struct Win;

/// Name the process for Windows (taskbar grouping and notifications). Call once at startup.
pub fn set_app_id() {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    // SAFETY: plain call with a valid string.
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `explorer` with one argument passed exactly as written (it does its own, unusual parsing).
fn explorer(arg: &str) -> io::Result<()> {
    Command::new("explorer").raw_arg(arg).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ())
}

impl Platform for Win {
    fn memory_extra(&self) -> Option<MemExtra> {
        use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        ms.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let mut pi: PERFORMANCE_INFORMATION = unsafe { std::mem::zeroed() };
        pi.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
        // SAFETY: both structs are sized as the calls require.
        if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 || unsafe { GetPerformanceInfo(&mut pi, pi.cb) } == 0 {
            return None;
        }
        let page = pi.PageSize as u64;
        let used = ms.ullTotalPhys.saturating_sub(ms.ullAvailPhys);
        let wired = pi.KernelNonpaged as u64 * page;
        let cached = (pi.SystemCache as u64 * page).min(ms.ullAvailPhys);
        let swap_total = ms.ullTotalPageFile.saturating_sub(ms.ullTotalPhys);
        let committed = ms.ullTotalPageFile.saturating_sub(ms.ullAvailPageFile);
        Some(MemExtra {
            app: used.saturating_sub(wired),
            wired,
            compressed: 0,
            cached,
            used,
            pressure: match ms.dwMemoryLoad {
                90.. => Pressure::Critical,
                80.. => Pressure::Warn,
                _ => Pressure::Normal,
            },
            swap_used: committed.saturating_sub(used).min(swap_total),
            swap_total,
        })
    }

    fn available_for_important_usage(&self, _: &Path) -> Option<u64> {
        None // Windows has no purgeable space
    }

    fn is_user_volume(&self, _: &Path, _: &str) -> bool {
        true
    }

    fn volume_label(&self, mount: &Path, fallback: &str) -> String {
        use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
        let root = wide(&mount.to_string_lossy());
        let mut name = [0u16; 261];
        // SAFETY: NUL-terminated root, a sized buffer, and null for the outputs we do not need.
        let ok = unsafe {
            GetVolumeInformationW(root.as_ptr(), name.as_mut_ptr(), name.len() as u32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0)
        } != 0;
        let label = if ok { String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(0)]) } else { String::new() };
        let drive = mount.to_string_lossy().trim_end_matches('\\').to_string();
        match (label.is_empty(), drive.is_empty()) {
            (false, false) => format!("{label} ({drive})"),
            (true, false) => format!("Local Disk ({drive})"),
            _ => fallback.to_string(),
        }
    }

    fn reveal(&self, path: &Path) -> io::Result<()> {
        explorer(&format!("/select,\"{}\"", path.display()))
    }

    fn open(&self, path: &Path) -> io::Result<()> {
        explorer(&format!("\"{}\"", path.display()))
    }

    /// Windows has no Quick Look: the file opens in its usual app.
    fn quick_look(&self, path: &Path) -> io::Result<()> {
        self.open(path)
    }

    fn open_full_disk_access_settings(&self) -> io::Result<()> {
        Ok(()) // no such permission on Windows
    }

    fn full_disk_access(&self) -> Option<bool> {
        None
    }

    fn notify(&self, title: &str, body: &str) {
        let _ = self.notify_checked(title, body);
    }

    fn notify_checked(&self, title: &str, body: &str) -> Result<(), String> {
        use windows::core::HSTRING;
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let xml = format!("<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>", esc(title), esc(body));
        let run = || -> windows::core::Result<()> {
            let doc = XmlDocument::new()?;
            doc.LoadXml(&HSTRING::from(xml))?;
            let toast = ToastNotification::CreateToastNotification(&doc)?;
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)
        };
        run().map_err(|e| format!("Windows did not show the notification: {}", e.message()))
    }

    /// Settings → Apps → Startup.
    fn open_login_items_settings(&self) -> io::Result<()> {
        explorer("ms-settings:startupapps")
    }

    /// Settings → System → Notifications.
    fn open_notification_settings(&self) -> io::Result<()> {
        explorer("ms-settings:notifications")
    }

    /// Only a list of names fits in a message box, so every app in it is quit; nothing can be kept open.
    fn choose_from_list(&self, title: &str, _prompt: &str, items: &[String], ok: &str) -> Option<Vec<String>> {
        let msg = format!("{}\n\nSave your work first if any of them asks.", items.join(", "));
        self.confirm(title, &msg, ok).then(Vec::new)
    }

    fn sleep_now(&self) -> io::Result<()> {
        use windows_sys::Win32::System::Power::SetSuspendState;
        // SAFETY: plain call; sleep (not hibernate), wake events allowed.
        if unsafe { SetSuspendState(false, false, false) } { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    fn lock_screen(&self) -> io::Result<()> {
        use windows_sys::Win32::System::Shutdown::LockWorkStation;
        // SAFETY: plain call.
        if unsafe { LockWorkStation() } != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    /// `shutdown` without /f: open apps are asked to close and can still stop it to save their work.
    fn power(&self, what: Power) -> Result<(), String> {
        let flag = match what {
            Power::Restart => "/r",
            Power::ShutDown => "/s",
        };
        let ok = Command::new("shutdown").args([flag, "/t", "0"]).creation_flags(CREATE_NO_WINDOW).status().map_err(|e| e.to_string())?;
        if ok.success() { Ok(()) } else { Err("Windows refused.".into()) }
    }

    /// USB sticks, memory cards and mounted disk images (never the Windows drive).
    fn ejectable_disks(&self) -> Vec<Disk> {
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_CDROM: u32 = 5;
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase();
        // SAFETY: plain call; a bit per drive letter.
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| format!("{}:", (b'A' + i) as char))
            .filter(|d| *d != system)
            .filter(|d| {
                let root = format!("{d}\\");
                // SAFETY: NUL-terminated path.
                let kind = unsafe { GetDriveTypeW(wide(&root).as_ptr()) };
                // an empty card reader or DVD drive has no volume and nothing to eject
                (kind == DRIVE_REMOVABLE || kind == DRIVE_CDROM) && Path::new(&root).exists()
            })
            .map(|d| Disk { name: self.volume_label(Path::new(&format!("{d}\\")), &d), path: PathBuf::from(format!("{d}\\")) })
            .collect()
    }

    /// File Explorer's own Eject, which also flushes the drive.
    fn eject(&self, disk: &Disk) -> Result<(), String> {
        let letter = disk.path.to_string_lossy().trim_end_matches('\\').to_string();
        let script = format!("(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{letter}').InvokeVerb('Eject'); Start-Sleep -Seconds 2");
        let _ = powershell(&script).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if Path::new(&disk.path).exists() {
            Err(format!("{} is still in use.", disk.name))
        } else {
            Ok(())
        }
    }

    /// A hidden PowerShell that holds Windows awake (`SetThreadExecutionState`) until it is killed, the time
    /// runs out, or this app exits.
    fn keep_awake(&self, display: bool, secs: Option<u64>) -> io::Result<std::process::Child> {
        // ES_CONTINUOUS | ES_SYSTEM_REQUIRED, plus ES_DISPLAY_REQUIRED
        let flags: u32 = 0x8000_0000 | 0x1 | if display { 0x2 } else { 0 };
        let timeout = secs.map(|s| format!(" -Timeout {s}")).unwrap_or_default();
        let script = format!(
            "$k = Add-Type -Name P -Namespace DeskveetAwake -PassThru -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint f);'; \
             [void]$k::SetThreadExecutionState({flags}); Wait-Process -Id {}{timeout} -ErrorAction SilentlyContinue",
            std::process::id()
        );
        powershell(&script).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    }

    /// A warning box with Cancel as the default button. Windows message boxes label their own buttons.
    fn confirm(&self, title: &str, message: &str, ok: &str) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST};
        let text = wide(&format!("{message}\n\nChoose OK to {}.", ok.to_lowercase()));
        let cap = wide(title);
        // SAFETY: NUL-terminated strings; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), cap.as_ptr(), MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND) == IDOK }
    }

    fn pick_save_file(&self, default_name: &str) -> Option<PathBuf> {
        dialog::save("Export custom rules", default_name)
    }

    fn pick_file(&self) -> Option<PathBuf> {
        dialog::open("Import custom rules", false)
    }

    fn pick_folder(&self) -> Option<PathBuf> {
        dialog::open("Choose a folder to scan", true)
    }
}

/// The standard Windows file and folder pickers.
mod dialog {
    use std::path::PathBuf;
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};

    fn result(d: &IFileDialog) -> Option<PathBuf> {
        // SAFETY: the dialog was shown and returned OK; the returned string is freed below.
        unsafe {
            let item = d.GetResult().ok()?;
            let p: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.map(PathBuf::from)
        }
    }

    pub fn open(title: &str, folders: bool) -> Option<PathBuf> {
        // SAFETY: COM is set up for this thread before the dialog is created; errors and Cancel give None.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let o = d.GetOptions().ok()?;
            d.SetOptions(o | FOS_FORCEFILESYSTEM | if folders { FOS_PICKFOLDERS } else { Default::default() }).ok()?;
            d.SetTitle(&HSTRING::from(title)).ok()?;
            d.Show(None).ok()?;
            result(&d.into())
        }
    }

    pub fn save(title: &str, name: &str) -> Option<PathBuf> {
        // SAFETY: as above.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            d.SetTitle(&HSTRING::from(title)).ok()?;
            d.SetFileName(&HSTRING::from(name)).ok()?;
            d.Show(None).ok()?;
            result(&d.into())
        }
    }
}

/// Windows processes that keep the system and the desktop running.
const PROTECTED_NAMES: &[&str] = &[
    "system", "registry", "memory compression", "secure system", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe",
    "lsass.exe", "lsaiso.exe", "svchost.exe", "dwm.exe", "fontdrvhost.exe", "sihost.exe", "ctfmon.exe", "explorer.exe", "taskhostw.exe",
    "runtimebroker.exe", "searchhost.exe", "startmenuexperiencehost.exe", "shellexperiencehost.exe", "textinputhost.exe", "spoolsv.exe",
    "audiodg.exe", "conhost.exe", "msmpeng.exe", "securityhealthservice.exe", "wudfhost.exe", "dllhost.exe",
];

impl ProcessInfo for Win {
    fn protected_reason(&self, p: &ProcFacts, me_uid: u32, me_pid: u32) -> Option<&'static str> {
        if p.pid <= 4 {
            return Some("system process");
        }
        if p.pid == me_pid {
            return Some("this app");
        }
        if p.uid != me_uid {
            return Some("owned by another user or by Windows");
        }
        if PROTECTED_NAMES.contains(&p.name.to_lowercase().as_str()) {
            return Some("part of Windows");
        }
        if let Some(exe) = p.exe {
            let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase();
            if exe.to_string_lossy().to_lowercase().starts_with(&windir) {
                return Some("part of Windows");
            }
        }
        None
    }

    fn reveal_target(&self, exe: &Path) -> PathBuf {
        exe.to_path_buf()
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Deskveet";

/// Whether Deskveet starts when this user signs in (its value in the user's Run key points at this program).
pub fn opens_at_login() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated names; the buffer size is passed in bytes.
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(RUN_VALUE).as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
    } == 0;
    let me = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    ok && String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]).to_lowercase().contains(&me)
}

/// Add or remove Deskveet from the user's Run key (what Settings → Apps → Startup lists and switches).
pub fn set_open_at_login(on: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ};
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: NUL-terminated names; the key is closed below.
    unsafe {
        let r = RegCreateKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut());
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r as i32).to_string());
        }
        let r = if on {
            let cmd = wide(&format!("\"{}\"", exe.display()));
            RegSetValueExW(key, wide(RUN_VALUE).as_ptr(), 0, REG_SZ, cmd.as_ptr().cast(), (cmd.len() * 2) as u32)
        } else {
            match RegDeleteValueW(key, wide(RUN_VALUE).as_ptr()) {
                2 => 0, // ERROR_FILE_NOT_FOUND: already off
                r => r,
            }
        };
        RegCloseKey(key);
        if r == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(r as i32).to_string()) }
    }
}

/// A hidden PowerShell running `script` (passed encoded, so no quoting can break it).
fn powershell(script: &str) -> Command {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand", &crate::apps::base64(&utf16)]).creation_flags(CREATE_NO_WINDOW);
    c
}

/// The full path of a running program, if Windows lets this user see it.
fn exe_path(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is closed below; the buffer length is passed in and updated.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize])))
    }
}

/// The program's own name for itself ("Google Chrome" for chrome.exe), from its version resource.
fn file_description(exe: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let path = wide(&exe.to_string_lossy());
    // SAFETY: buffers are sized by the first call; VerQueryValueW points into `data`, which outlives its use.
    unsafe {
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut langs = vec![0x0409_04b0u32]; // US English, Unicode: the usual fallback
        if VerQueryValueW(data.as_ptr().cast(), wide(r"\VarFileInfo\Translation").as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, len as usize / 2);
            langs = pairs.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0] as u32) << 16 | c[1] as u32).chain(langs).collect();
        }
        for l in langs {
            let key = wide(&format!(r"\StringFileInfo\{l:08x}\FileDescription"));
            if VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut ptr, &mut len) != 0 && len > 1 {
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr as *const u16, len as usize - 1)).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
}

/// Programs that own a window on the taskbar but are part of Windows itself.
const SHELL_EXES: &[&str] = &["explorer.exe", "applicationframehost.exe", "textinputhost.exe", "shellexperiencehost.exe", "searchhost.exe", "startmenuexperiencehost.exe", "systemsettings.exe"];

/// Processes with a visible top-level window, as the taskbar shows them, first window first.
fn window_pids() -> Vec<u32> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
    };
    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> i32 {
        // SAFETY: called by EnumWindows with the pointer passed below, which lives for the whole call.
        unsafe {
            let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
            if IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() && !tool && GetWindowTextLengthW(hwnd) > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                let v = &mut *(out as *mut Vec<u32>);
                if pid != 0 && !v.contains(&pid) {
                    v.push(pid);
                }
            }
        }
        1
    }
    let mut v: Vec<u32> = Vec::new();
    // SAFETY: `v` outlives the call; the callback only pushes to it.
    unsafe { EnumWindows(Some(each), &mut v as *mut Vec<u32> as LPARAM) };
    v
}

impl Apps for Win {
    /// Apps with a window on the taskbar. The id is the program's path; its folder groups helper processes,
    /// except for programs that live in the Windows folder (they share it with everything else there).
    fn running_apps(&self) -> Vec<RunningApp> {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase();
        let mut apps: Vec<RunningApp> = window_pids()
            .into_iter()
            .filter_map(|pid| {
                let exe = exe_path(pid)?;
                let file = exe.file_name()?.to_string_lossy().to_lowercase();
                if SHELL_EXES.contains(&file.as_str()) {
                    return None;
                }
                let lower = exe.to_string_lossy().to_lowercase();
                let name = file_description(&exe).unwrap_or_else(|| exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                let folder = exe.parent().filter(|_| !lower.starts_with(&windir)).map(Path::to_path_buf);
                Some(RunningApp { pid, name, bundle_id: lower, bundle_path: folder })
            })
            .collect();
        // one row per program: several windows of one app can belong to several processes
        let mut seen = std::collections::HashSet::new();
        apps.retain(|a| seen.insert(a.bundle_id.clone()));
        apps
    }
    fn app_icon_png(&self, _: &Path) -> Option<Vec<u8>> {
        None
    }
    fn quit_app(&self, pid: u32, force: bool) -> bool {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string()]).creation_flags(CREATE_NO_WINDOW);
        if force {
            c.arg("/F");
        }
        c.status().is_ok_and(|s| s.success())
    }
    fn responsible_pid(&self, _: u32) -> Option<u32> {
        None
    }
}
