//! Every side effect of cleaning (processes, lsof, tools, Trash) behind one trait, so the engine can be
//! tested with a fake and unit tests can never touch the real Trash or run real tools.

use crate::fsutil;
use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::OnceLock;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cmd: String,
}

pub trait Sys: Send + Sync {
    /// Run a registry `detect` test (`sh -c`). True when it exits 0.
    fn detect(&self, cmd: &str) -> bool;
    /// Run a read-only command and return stdout.
    fn capture(&self, argv: &[String]) -> Result<String, String>;
    /// Run a cleaning tool; returns the tail of its output.
    fn run_tool(&self, argv: &[String]) -> Result<String, String>;
    fn processes(&self) -> Vec<ProcInfo>;
    /// (pid, command, path) of every file the user's processes hold open.
    fn open_files(&self) -> Vec<(u32, String, PathBuf)>;
    /// Move to the Trash. Returns where it ended up when the OS tells us.
    fn trash(&self, path: &Path) -> io::Result<Option<PathBuf>>;
    fn free_bytes(&self, path: &Path) -> u64 {
        fsutil::free_bytes(path)
    }
    /// Environment to resolve tool locations with.
    fn env(&self) -> HashMap<String, String> {
        std::env::vars().collect()
    }
    /// Is an app with this bundle id installed? `None` when the platform can't tell.
    fn app_installed(&self, _bundle_id: &str) -> Option<bool> {
        None
    }
    /// A quick read-only question to a tool (`go env GOCACHE`). `None` when it is missing or fails.
    fn query(&self, argv: &[String]) -> Option<String> {
        self.capture(argv).ok()
    }
}

/// PATH for tools. Apps started from the Dock get a minimal PATH, so add the usual developer locations.
pub fn tool_path(home: &Path) -> String {
    let mut v: Vec<String> = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"].iter().map(|s| s.to_string()).collect();
    for rel in [".cargo/bin", "go/bin", ".pub-cache/bin", ".volta/bin", ".bun/bin", ".local/bin", "Library/pnpm", ".yarn/bin", "development/flutter/bin"] {
        v.push(home.join(rel).to_string_lossy().into_owned());
    }
    // nvm: newest installed node last in lexical order is good enough
    if let Ok(rd) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut vers: Vec<_> = rd.flatten().map(|e| e.path().join("bin")).collect();
        vers.sort();
        if let Some(last) = vers.pop() {
            v.push(last.to_string_lossy().into_owned());
        }
    }
    if let Ok(p) = std::env::var("PATH") {
        v.push(p);
    }
    v.join(":")
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn exec(mut cmd: Command, timeout: Duration) -> Result<Out, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let t1 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t2 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("timed out".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => break Err(e.to_string()),
        }
    };
    let (b1, b2) = (t1.join().unwrap_or_default(), t2.join().unwrap_or_default());
    let status = status?;
    Ok(Out { ok: status.success(), stdout: String::from_utf8_lossy(&b1).into_owned(), stderr: String::from_utf8_lossy(&b2).into_owned() })
}

fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.len() <= n {
        t.to_string()
    } else {
        let mut start = t.len() - n;
        while !t.is_char_boundary(start) {
            start += 1;
        }
        format!("…{}", &t[start..])
    }
}

pub struct RealSys {
    home: PathBuf,
    login_env: OnceLock<HashMap<String, String>>,
}

/// The user's login shell from the password database (not always zsh).
fn login_shell() -> String {
    let mut buf = [0 as libc::c_char; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: buffers sized as passed; `res` only read on success.
    let rc = unsafe { libc::getpwuid_r(libc::getuid(), &mut pwd, buf.as_mut_ptr(), buf.len(), &mut res) };
    if rc == 0 && !res.is_null() && !pwd.pw_shell.is_null() {
        let s = unsafe { std::ffi::CStr::from_ptr(pwd.pw_shell) }.to_string_lossy().into_owned();
        if s.starts_with('/') {
            return s;
        }
    }
    "/bin/zsh".into()
}

/// Total bytes of the volume holding `path`.
pub fn volume_total(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return 0 };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    st.f_blocks as u64 * st.f_frsize as u64
}

/// Commands that, on a Mac without Xcode or the Command Line Tools, are stubs that pop up an
/// "install developer tools" dialog instead of running. They are only run when the tools exist.
const DEV_TOOL_SHIMS: &[&str] = &["xcrun", "python3", "pip3", "git", "clang", "make", "swift", "xcodebuild"];

fn developer_tools_installed() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| {
        // `xcode-select -p` only prints the folder; it never shows the install dialog.
        Command::new("/usr/bin/xcode-select")
            .arg("-p")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir())
            .unwrap_or(false)
    })
}

/// True when running `program` could trigger the developer-tools install dialog.
pub fn would_prompt_for_dev_tools(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    DEV_TOOL_SHIMS.contains(&name) && (program == name || program.starts_with("/usr/bin/")) && !developer_tools_installed()
}

/// Parse `env -0` output. Lines a profile script printed before it are ignored.
pub fn parse_env0(out: &[u8]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for (i, entry) in out.split(|b| *b == 0).enumerate() {
        let e = String::from_utf8_lossy(entry);
        // anything a profile script printed lands before the first key
        let e = if i == 0 { e.rsplit('\n').next().unwrap_or(&e) } else { &e };
        if let Some((k, v)) = e.split_once('=') {
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()) {
                m.insert(k.to_string(), v.to_string());
            }
        }
    }
    m
}

impl RealSys {
    /// Process names only (cheap), for busy checks during live re-measuring.
    pub fn processes_light(&self) -> Vec<ProcInfo> {
        self.processes()
    }

    pub fn new(home: PathBuf) -> Self {
        RealSys { home, login_env: OnceLock::new() }
    }

    /// Environment of a fresh login shell, read once (apps from the Dock don't inherit shell variables).
    fn login(&self) -> &HashMap<String, String> {
        self.login_env.get_or_init(|| {
            let mut c = Command::new(login_shell());
            c.args(["-l", "-c", "env -0"]).current_dir(&self.home);
            exec(c, Duration::from_secs(5)).ok().filter(|o| o.ok).map(|o| parse_env0(o.stdout.as_bytes())).unwrap_or_default()
        })
    }

    fn path(&self) -> String {
        match self.login().get("PATH") {
            Some(p) if !p.is_empty() => format!("{}:{p}", tool_path(&self.home)),
            _ => tool_path(&self.home),
        }
    }

    fn command(&self, argv: &[String]) -> Result<Command, String> {
        let (prog, args) = argv.split_first().ok_or("empty command")?;
        if would_prompt_for_dev_tools(prog) {
            return Err(format!("{prog} needs Xcode or the Command Line Tools, which are not installed"));
        }
        let mut c = Command::new(prog);
        c.args(args).env("PATH", self.path()).current_dir(&self.home);
        Ok(c)
    }
}

impl Sys for RealSys {
    fn env(&self) -> HashMap<String, String> {
        // The app's own variables win; the login shell fills in what the Dock didn't pass on.
        let mut m = self.login().clone();
        m.extend(std::env::vars());
        m
    }

    fn app_installed(&self, bundle_id: &str) -> Option<bool> {
        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::NSWorkspace;
            use objc2_foundation::NSString;
            objc2::rc::autoreleasepool(|_| Some(NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id)).is_some()))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = bundle_id;
            None
        }
    }

    fn query(&self, argv: &[String]) -> Option<String> {
        let o = exec(self.command(argv).ok()?, Duration::from_secs(8)).ok()?;
        o.ok.then_some(o.stdout)
    }

    fn detect(&self, cmd: &str) -> bool {
        if cmd.split_whitespace().any(would_prompt_for_dev_tools) {
            return false;
        }
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(cmd).env("PATH", self.path()).current_dir(&self.home);
        exec(c, Duration::from_secs(10)).map(|o| o.ok).unwrap_or(false)
    }

    fn capture(&self, argv: &[String]) -> Result<String, String> {
        let o = exec(self.command(argv)?, Duration::from_secs(60))?;
        if o.ok {
            Ok(o.stdout)
        } else {
            Err(tail(&o.stderr, 400))
        }
    }

    fn run_tool(&self, argv: &[String]) -> Result<String, String> {
        let o = exec(self.command(argv)?, Duration::from_secs(20 * 60))?;
        if o.ok {
            Ok(tail(&format!("{}\n{}", o.stdout, o.stderr), 600))
        } else {
            Err(tail(if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr }, 600))
        }
    }

    fn processes(&self) -> Vec<ProcInfo> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always).with_cmd(UpdateKind::Always),
        );
        sys.processes()
            .iter()
            .map(|(pid, p)| ProcInfo {
                pid: pid.as_u32(),
                name: p.exe().and_then(|e| e.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.name().to_string_lossy().into_owned()),
                cmd: p.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
            })
            .collect()
    }

    fn open_files(&self) -> Vec<(u32, String, PathBuf)> {
        let mut c = Command::new("/usr/sbin/lsof");
        c.args(["-nP", "-w", "-F", "pcn"]);
        let Ok(o) = exec(c, Duration::from_secs(25)) else { return Vec::new() };
        let (mut pid, mut cmd) = (0u32, String::new());
        let mut out = Vec::new();
        for line in o.stdout.lines() {
            let (tag, rest) = line.split_at(1.min(line.len()));
            match tag {
                "p" => pid = rest.parse().unwrap_or(0),
                "c" => cmd = rest.to_string(),
                "n" if rest.starts_with('/') => out.push((pid, cmd.clone(), PathBuf::from(rest))),
                _ => {}
            }
        }
        out
    }

    fn trash(&self, path: &Path) -> io::Result<Option<PathBuf>> {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSFileManager, NSString, NSURL};
            // NSFileManager needs no Automation permission (the Finder method would prompt for one).
            let p = path.to_str().ok_or_else(|| io::Error::other("path is not valid UTF-8"))?;
            objc2::rc::autoreleasepool(|_| {
                let url = NSURL::fileURLWithPath(&NSString::from_str(p));
                let mut resulting = None;
                NSFileManager::defaultManager()
                    .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
                    .map_err(|e| io::Error::other(e.to_string()))?;
                Ok(resulting.and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string())))
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            trash::delete(path).map_err(|e| io::Error::other(e.to_string()))?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env0_parsing_ignores_profile_noise_and_bad_keys() {
        let out = b"Welcome to zsh!\nHOME=/Users/x\0GRADLE_USER_HOME=/Volumes/Dev/gradle\0weird key=1\0PATH=/a:/b\0MULTI=line1\nline2\0";
        let m = parse_env0(out);
        assert_eq!(m.get("HOME").map(String::as_str), Some("/Users/x"));
        assert_eq!(m.get("GRADLE_USER_HOME").map(String::as_str), Some("/Volumes/Dev/gradle"));
        assert_eq!(m.get("PATH").map(String::as_str), Some("/a:/b"));
        assert!(!m.contains_key("weird key"));
        assert_eq!(m.get("MULTI").map(String::as_str), Some("line1\nline2"));
    }

    #[test]
    fn developer_tool_shims_are_recognised() {
        // On this Mac the tools exist, so nothing is blocked; the classification itself is what matters.
        let installed = developer_tools_installed();
        assert_eq!(would_prompt_for_dev_tools("xcrun"), !installed);
        assert_eq!(would_prompt_for_dev_tools("/usr/bin/python3"), !installed);
        assert!(!would_prompt_for_dev_tools("/opt/homebrew/bin/python3"), "a real python is never a shim");
        assert!(!would_prompt_for_dev_tools("brew"));
    }

    #[test]
    fn login_shell_environment_is_readable() {
        let s = RealSys::new(std::env::var_os("HOME").unwrap().into());
        let env = s.env();
        assert!(env.contains_key("HOME") && env.contains_key("PATH"));
    }
}
