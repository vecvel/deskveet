//! The only surface the UI can call. Every command re-validates its inputs.

use crate::cleaner::{ArtifactScan, Engine, Plan, Report, RuleResult, RunOpts};
use crate::platform::{self, Platform};
use crate::rules::Registry;
use crate::scan::{self, Ctx, Meta, Opts, Row};
use crate::sys::RealSys;
use crate::undo::Entry;
use crate::sysmon::{Monitor, Overview, ProcList, ProcQuery};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::collections::{HashSet, VecDeque};
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
    engine: Arc<Engine>,
    results: Mutex<Arc<Vec<RuleResult>>>,
    art_results: Mutex<Arc<Vec<RuleResult>>>,
    art_scan: Mutex<Option<ArtifactScan>>,
    plans: Mutex<VecDeque<(String, Arc<Plan>)>>,
    /// A cleanup scan or run is in progress.
    cleaning: AtomicBool,
    clean_cancel: Arc<AtomicBool>,
    /// Small UI preferences (last tab, folders, view modes). Kept here, not in the webview, so they
    /// survive the window being released and recreated.
    ui: Mutex<std::collections::HashMap<String, serde_json::Value>>,
}

fn ui_path() -> PathBuf {
    platform::data_dir().join("ui-state.json")
}

fn real_engine() -> Engine {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let reg = Registry::macos().expect("the shipped rules file is validated by tests");
    Engine::new(reg, home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), platform::data_dir().join("undo-log.json"))
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mon: Mutex::new(Monitor::new()),
            loaded: Mutex::new(None),
            running: Mutex::new(None),
            engine: Arc::new(real_engine()),
            results: Mutex::new(Arc::new(Vec::new())),
            art_results: Mutex::new(Arc::new(Vec::new())),
            art_scan: Mutex::new(None),
            plans: Mutex::new(VecDeque::new()),
            cleaning: AtomicBool::new(false),
            clean_cancel: Arc::new(AtomicBool::new(false)),
            ui: Mutex::new(std::fs::read(ui_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()),
        }
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

// ───────────────────────────── cleanup ─────────────────────────────

#[derive(Serialize)]
pub struct CleanupState {
    pub running: bool,
    pub rules_version: String,
    pub results: Vec<RuleResult>,
    pub undo_log: String,
}

#[derive(Serialize, Clone)]
struct CleanupDone {
    cancelled: bool,
}

#[derive(Serialize, Clone)]
struct CleanProgress {
    done: usize,
    total: usize,
    current: String,
}

#[tauri::command(async)]
pub fn cleanup_state(state: State<'_, AppState>) -> CleanupState {
    CleanupState {
        running: state.cleaning.load(Relaxed),
        rules_version: state.engine.reg.rules_version.clone(),
        results: state.results.lock().unwrap().as_ref().clone(),
        undo_log: state.engine.undo.path().to_string_lossy().into_owned(),
    }
}

/// Scan all rules in the background; each finished rule is pushed to the UI as `cleanup-rule`.
#[tauri::command(async)]
pub fn cleanup_scan(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.cleaning.swap(true, Relaxed) {
        return Err("Cleanup is already busy.".into());
    }
    state.clean_cancel.store(false, Relaxed);
    *state.results.lock().unwrap() = Arc::new(Vec::new());
    let eng = state.engine.clone();
    let cancel = state.clean_cancel.clone();
    std::thread::Builder::new()
        .name("cleanup-scan".into())
        .spawn(move || {
            let emit_app = app.clone();
            let res = eng.scan_all(&cancel, &move |rr| {
                let _ = emit_app.emit("cleanup-rule", rr);
            });
            let st = app.state::<AppState>();
            if !cancel.load(Relaxed) {
                *st.results.lock().unwrap() = Arc::new(res);
            }
            st.cleaning.store(false, Relaxed);
            let _ = app.emit("cleanup-done", CleanupDone { cancelled: cancel.load(Relaxed) });
            platform::trim_memory();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn cleanup_cancel(state: State<'_, AppState>) {
    state.clean_cancel.store(true, Relaxed);
}

#[derive(serde::Deserialize)]
pub struct PlanReq {
    pub ids: Vec<String>,
    #[serde(default)]
    pub picks: Vec<String>,
    pub dry_run: bool,
}

/// Build the verified preview. Rule items are referenced by the ids our own scan produced.
#[tauri::command(async)]
pub fn cleanup_plan(state: State<'_, AppState>, req: PlanReq) -> Result<Plan, String> {
    if state.cleaning.load(Relaxed) {
        return Err("Wait for the current scan or clean to finish.".into());
    }
    let mut results = state.results.lock().unwrap().as_ref().clone();
    results.extend(state.art_results.lock().unwrap().iter().cloned());
    let plan = state.engine.plan(&results, &req.ids, &req.picks, req.dry_run);
    if plan.items.is_empty() {
        return Err("Nothing to clean.".into());
    }
    let mut plans = state.plans.lock().unwrap();
    plans.push_back((plan.token.clone(), Arc::new(plan.clone())));
    while plans.len() > 4 {
        plans.pop_front();
    }
    Ok(plan)
}

#[derive(serde::Deserialize)]
pub struct RunReq {
    pub token: String,
    #[serde(default)]
    pub confirm_erase: bool,
    #[serde(default)]
    pub overrides: Vec<String>,
}

#[tauri::command(async)]
pub fn cleanup_run(app: AppHandle, state: State<'_, AppState>, req: RunReq) -> Result<Report, String> {
    let plan = {
        let mut plans = state.plans.lock().unwrap();
        let i = plans.iter().position(|(t, _)| *t == req.token).ok_or("This preview has expired. Preview again.")?;
        plans.remove(i).map(|(_, p)| p).unwrap()
    };
    if state.cleaning.swap(true, Relaxed) {
        return Err("Cleanup is already busy.".into());
    }
    // Only ids that are part of the plan can be overridden.
    let in_plan: HashSet<&str> = plan.items.iter().map(|i| i.id.as_str()).collect();
    let opts = RunOpts { confirm_erase: req.confirm_erase, overrides: req.overrides.iter().filter(|o| in_plan.contains(o.as_str())).cloned().collect() };
    let eng = state.engine.clone();
    let emit_app = app.clone();
    let report = eng.run(&plan, &opts, &move |done, total, current| {
        let _ = emit_app.emit("cleanup-progress", CleanProgress { done, total, current: current.to_string() });
    });
    if !report.dry_run {
        // what we scanned is out of date now, and so is the storage tree
        *state.results.lock().unwrap() = Arc::new(Vec::new());
        *state.art_results.lock().unwrap() = Arc::new(Vec::new());
        *state.art_scan.lock().unwrap() = None;
        *state.loaded.lock().unwrap() = None;
    }
    state.cleaning.store(false, Relaxed);
    platform::trim_memory();
    Ok(report)
}

#[tauri::command(async)]
pub fn undo_list(state: State<'_, AppState>) -> Vec<Entry> {
    state.engine.undo.list()
}

#[tauri::command(async)]
pub fn undo_restore(state: State<'_, AppState>, id: String) -> Result<String, String> {
    state.engine.restore(&id)
}

#[tauri::command(async)]
pub fn undo_clear(state: State<'_, AppState>) -> Result<(), String> {
    state.engine.undo.clear().map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn open_trash(state: State<'_, AppState>) -> Result<(), String> {
    let t = state.engine.trash_dir.clone();
    state.mon.lock().unwrap().plat().open(&t).map_err(|e| e.to_string())
}

// ───────────────────────────── project artifacts ─────────────────────────────

#[derive(Serialize)]
pub struct ArtifactState {
    /// Some cleanup scan or clean (caches or artifacts) is in progress; only one runs at a time.
    pub busy: bool,
    pub default_roots: Vec<String>,
    pub scan: Option<ArtifactScan>,
}

#[derive(Serialize, Clone)]
struct ArtifactProgress {
    dirs: u64,
}

#[tauri::command(async)]
pub fn artifact_state(state: State<'_, AppState>) -> ArtifactState {
    let home = state.engine.home().to_path_buf();
    let default_roots = ["Desktop", "Documents", "development", "Developer", "Projects", "dev", "code", "src", "work", "repos"]
        .iter()
        .map(|d| home.join(d))
        .filter(|p| p.is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    ArtifactState { busy: state.cleaning.load(Relaxed), default_roots, scan: state.art_scan.lock().unwrap().clone() }
}

/// Look for build folders below the folders the user chose. Result arrives as `artifact-done`.
#[tauri::command(async)]
pub fn artifact_scan(app: AppHandle, state: State<'_, AppState>, roots: Vec<String>) -> Result<(), String> {
    let home = state.engine.home().to_path_buf();
    let roots: Vec<PathBuf> = roots
        .iter()
        .map(PathBuf::from)
        .filter(|r| r.is_absolute() && r.is_dir())
        .filter(|r| r.canonicalize().map(|c| c.starts_with(&home) || c.starts_with("/Volumes")).unwrap_or(false))
        .collect();
    if roots.is_empty() {
        return Err("Pick at least one folder inside your home folder or on an external volume.".into());
    }
    if state.cleaning.swap(true, Relaxed) {
        return Err("Cleanup is already busy.".into());
    }
    state.clean_cancel.store(false, Relaxed);
    let eng = state.engine.clone();
    let cancel = state.clean_cancel.clone();
    std::thread::Builder::new()
        .name("artifact-scan".into())
        .spawn(move || {
            let seen = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let finished = Arc::new(AtomicBool::new(false));
            let ticker = {
                let (app, seen, finished) = (app.clone(), seen.clone(), finished.clone());
                std::thread::spawn(move || {
                    while !finished.load(Relaxed) {
                        let _ = app.emit("artifact-progress", ArtifactProgress { dirs: seen.load(Relaxed) });
                        std::thread::sleep(Duration::from_millis(150));
                    }
                })
            };
            let (scan, results) = eng.find_artifacts(&roots, &cancel, &seen);
            finished.store(true, Relaxed);
            let _ = ticker.join();
            let st = app.state::<AppState>();
            if !cancel.load(Relaxed) {
                *st.art_results.lock().unwrap() = Arc::new(results);
                *st.art_scan.lock().unwrap() = Some(scan.clone());
            }
            st.cleaning.store(false, Relaxed);
            let _ = app.emit("artifact-done", scan);
            platform::trim_memory();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ───────────────────────────── UI state ─────────────────────────────

#[tauri::command(async)]
pub fn ui_get_all(state: State<'_, AppState>) -> std::collections::HashMap<String, serde_json::Value> {
    state.ui.lock().unwrap().clone()
}

#[tauri::command(async)]
pub fn ui_set(state: State<'_, AppState>, key: String, value: serde_json::Value) {
    if key.len() > 64 || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return;
    }
    let mut ui = state.ui.lock().unwrap();
    ui.insert(key, value);
    let path = ui_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(&*ui).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

// ───────────────────────────── about / settings ─────────────────────────────

#[derive(Serialize)]
pub struct About {
    pub version: String,
    pub rules_version: String,
    pub rules_count: usize,
    pub cache_dir: String,
    pub cache_bytes: u64,
    pub data_dir: String,
    pub undo_log: String,
}

#[tauri::command(async)]
pub fn about(state: State<'_, AppState>) -> About {
    let cache = platform::cache_dir();
    // WebKit's own cache lives in the same folder; only our scan files are ours to size and clear.
    let cache_bytes = std::fs::read_dir(&cache)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("scan-")).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0);
    About {
        version: env!("CARGO_PKG_VERSION").into(),
        rules_version: state.engine.reg.rules_version.clone(),
        rules_count: state.engine.reg.rules.len(),
        cache_dir: cache.to_string_lossy().into_owned(),
        cache_bytes,
        data_dir: platform::data_dir().to_string_lossy().into_owned(),
        undo_log: state.engine.undo.path().to_string_lossy().into_owned(),
    }
}

/// "granted" | "denied" | "unknown" for the build that is running right now.
#[tauri::command(async)]
pub fn fda_status(state: State<'_, AppState>) -> &'static str {
    match state.mon.lock().unwrap().plat().full_disk_access() {
        Some(true) => "granted",
        Some(false) => "denied",
        None => "unknown",
    }
}
