//! The only surface the UI can call. Every command re-validates its inputs.

use crate::platform::{self, Platform};
use crate::scan::{self, Ctx, Meta, Opts, Row};
use crate::sysmon::{Monitor, Overview, ProcList, ProcQuery};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

pub struct Loaded {
    pub root: PathBuf,
    pub meta: Meta,
    pub tree: scan::Node,
}

struct Running {
    root: String,
    cancel: Arc<AtomicBool>,
    ctx: Arc<Ctx>,
}

pub struct AppState {
    pub mon: Mutex<Monitor>,
    loaded: Mutex<Option<Arc<Loaded>>>,
    running: Mutex<Option<Arc<Running>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self { mon: Mutex::new(Monitor::new()), loaded: Mutex::new(None), running: Mutex::new(None) }
    }

    /// Drop everything that can be rebuilt; called when the window (and its webview) goes away.
    pub fn release_memory(&self) {
        if let Ok(mut m) = self.mon.lock() {
            m.release();
        }
        // The scan tree lives on in the disk cache; keep it in RAM only while it is being looked at.
        if self.running.lock().map(|r| r.is_none()).unwrap_or(true) {
            if let Ok(mut l) = self.loaded.lock() {
                *l = None;
            }
        }
        platform::trim_memory();
    }
}

// `async` makes Tauri run these on its thread pool instead of the main (UI) thread.
#[tauri::command(async)]
pub fn overview(state: State<'_, AppState>) -> Overview {
    state.mon.lock().unwrap().overview()
}

#[tauri::command(async)]
pub fn processes(state: State<'_, AppState>, query: ProcQuery) -> ProcList {
    state.mon.lock().unwrap().processes(&query)
}

#[tauri::command(async)]
pub fn quit_process(state: State<'_, AppState>, pid: u32, start: u64, force: bool) -> Result<(), String> {
    state.mon.lock().unwrap().signal(pid, start, force)
}

fn existing(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if !p.is_absolute() || !p.exists() {
        return Err("That path no longer exists.".into());
    }
    Ok(p)
}

#[tauri::command(async)]
pub fn reveal(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = existing(&path)?;
    state.mon.lock().unwrap().plat().reveal(&p).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn open_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = existing(&path)?;
    state.mon.lock().unwrap().plat().open(&p).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn quick_look(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = existing(&path)?;
    state.mon.lock().unwrap().plat().quick_look(&p).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn open_fda_settings(state: State<'_, AppState>) -> Result<(), String> {
    state.mon.lock().unwrap().plat().open_full_disk_access_settings().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn pick_folder(state: State<'_, AppState>) -> Option<String> {
    state.mon.lock().unwrap().plat().pick_folder().map(|p| p.to_string_lossy().into_owned())
}

/// The UI asks for this after it has been hidden for a while: closing the window frees the
/// webview and its helper processes (~70 MB). Dock click or Cmd+Tab brings it back.
#[tauri::command(async)]
pub fn release_window(window: tauri::WebviewWindow) {
    let _ = window.destroy();
}

// ───────────────────────────── storage scan ─────────────────────────────

#[derive(Serialize, Clone)]
pub struct Progress {
    pub root: String,
    pub files: u64,
    pub bytes: u64,
    pub dirs: u64,
    pub reused: u64,
    pub current: String,
}

#[derive(Serialize, Clone)]
pub struct Done {
    pub root: String,
    pub cancelled: bool,
    pub error: Option<String>,
    pub meta: Option<Meta>,
}

#[derive(Serialize)]
pub struct Status {
    pub home: String,
    pub running: Option<Progress>,
    pub loaded: Option<Meta>,
}

fn progress_of(r: &Running) -> Progress {
    Progress {
        root: r.root.clone(),
        files: r.ctx.files.load(Relaxed),
        bytes: r.ctx.bytes.load(Relaxed),
        dirs: r.ctx.dirs.load(Relaxed),
        reused: r.ctx.reused.load(Relaxed),
        current: r.ctx.current.lock().map(|c| c.clone()).unwrap_or_default(),
    }
}

#[tauri::command(async)]
pub fn scan_start(app: AppHandle, state: State<'_, AppState>, root: String, full: bool) -> Result<(), String> {
    let root = PathBuf::from(&root).canonicalize().map_err(|_| "That folder does not exist.".to_string())?;
    if !root.is_dir() {
        return Err("Pick a folder, not a file.".into());
    }
    let root_dev = scan::dev_of(&root).map_err(|e| e.to_string())?;
    let mut running = state.running.lock().unwrap();
    if running.is_some() {
        return Err("A scan is already running.".into());
    }
    let cache = platform::cache_dir();
    let prev = if full { None } else { scan::load_prev_index(&cache, &root, scan::now_secs()) };
    let writer = scan::IdxWriter::create(&scan::idx_path(&cache, &root)).ok(); // cache is best-effort
    let cancel = Arc::new(AtomicBool::new(false));
    let ctx = Arc::new(Ctx::new(cancel.clone(), prev, root_dev, Opts::default(), writer));
    let run = Arc::new(Running { root: root.to_string_lossy().into_owned(), cancel, ctx: ctx.clone() });
    *running = Some(run.clone());
    drop(running);

    std::thread::Builder::new()
        .name("scan-main".into())
        .spawn(move || {
            let t0 = Instant::now();
            let finished = Arc::new(AtomicBool::new(false));
            let ticker = {
                let (app, run, finished) = (app.clone(), run.clone(), finished.clone());
                std::thread::spawn(move || {
                    while !finished.load(Relaxed) {
                        let _ = app.emit("scan-progress", progress_of(&run));
                        std::thread::sleep(Duration::from_millis(120));
                    }
                })
            };

            let result = scan::run(&root, &ctx);
            finished.store(true, Relaxed);
            let _ = ticker.join();
            let index_saved = ctx.idx.as_ref().map(|w| w.finish(result.is_some()).is_ok()).unwrap_or(false);

            let state = app.state::<AppState>();
            let done = match result {
                Some(tree) => {
                    let meta = scan::meta_for(&root, &ctx, &tree, t0.elapsed().as_millis() as u64);
                    let error = scan::save_tree(&cache, &root, &meta, &tree)
                        .err()
                        .map(|e| format!("Could not save the scan cache: {e}"))
                        .or_else(|| (!index_saved).then(|| "Could not save the re-scan index.".to_string()));
                    *state.loaded.lock().unwrap() = Some(Arc::new(Loaded { root: root.clone(), meta: meta.clone(), tree }));
                    Done { root: run.root.clone(), cancelled: false, error, meta: Some(meta) }
                }
                None => Done { root: run.root.clone(), cancelled: run.cancel.load(Relaxed), error: (!run.cancel.load(Relaxed)).then(|| "The folder could not be read.".to_string()), meta: None },
            };
            *state.running.lock().unwrap() = None;
            let _ = app.emit("scan-done", done);
            platform::trim_memory();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn scan_cancel(state: State<'_, AppState>) {
    if let Some(r) = state.running.lock().unwrap().as_ref() {
        r.cancel.store(true, Relaxed);
    }
}

#[tauri::command(async)]
pub fn scan_status(state: State<'_, AppState>) -> Status {
    Status {
        home: std::env::var("HOME").unwrap_or_default(),
        running: state.running.lock().unwrap().as_ref().map(|r| progress_of(r)),
        loaded: state.loaded.lock().unwrap().as_ref().map(|l| l.meta.clone()),
    }
}

/// Load the cached scan for `root` (instant display before any re-scan). `None` if there is none.
#[tauri::command(async)]
pub fn scan_load_cached(state: State<'_, AppState>, root: String) -> Option<Meta> {
    let root = PathBuf::from(root).canonicalize().ok()?;
    if let Some(l) = state.loaded.lock().unwrap().as_ref() {
        if l.root == root {
            return Some(l.meta.clone());
        }
    }
    let (meta, tree) = scan::load_cache(&platform::cache_dir(), &root)?;
    *state.loaded.lock().unwrap() = Some(Arc::new(Loaded { root, meta: meta.clone(), tree }));
    Some(meta)
}

#[derive(Serialize)]
pub struct Children {
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub rows: Vec<Row>,
}

#[tauri::command(async)]
pub fn scan_children(state: State<'_, AppState>, path: String) -> Result<Children, String> {
    let loaded = state.loaded.lock().unwrap().clone().ok_or("No scan loaded.")?;
    let target = Path::new(&path);
    let node = scan::find(&loaded.tree, &loaded.root, target).ok_or("That folder is not part of the scan.")?;
    let rows = scan::rows(node, target);
    Ok(Children { size: node.size, files: node.files, rows, path })
}

#[tauri::command(async)]
pub fn scan_clear_cache(state: State<'_, AppState>) -> usize {
    *state.loaded.lock().unwrap() = None;
    scan::clear_cache(&platform::cache_dir())
}
