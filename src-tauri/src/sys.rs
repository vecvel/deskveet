//! Every side effect of cleaning (processes, lsof, tools, Trash) behind one trait, so the engine can be
//! tested with a fake and unit tests can never touch the real Trash or run real tools.

use crate::fsutil;
use std::io::{self, Read};
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
}

impl RealSys {
    pub fn new(home: PathBuf) -> Self {
        RealSys { home }
    }

    fn command(&self, argv: &[String]) -> Result<Command, String> {
        let (prog, args) = argv.split_first().ok_or("empty command")?;
        let mut c = Command::new(prog);
        c.args(args).env("PATH", tool_path(&self.home)).current_dir(&self.home);
        Ok(c)
    }
}

impl Sys for RealSys {
    fn detect(&self, cmd: &str) -> bool {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(cmd).env("PATH", tool_path(&self.home)).current_dir(&self.home);
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
            let url = NSURL::fileURLWithPath(&NSString::from_str(p));
            let mut resulting = None;
            NSFileManager::defaultManager()
                .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(resulting.and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string())))
        }
        #[cfg(not(target_os = "macos"))]
        {
            trash::delete(path).map_err(|e| io::Error::other(e.to_string()))?;
            Ok(None)
        }
    }
}
