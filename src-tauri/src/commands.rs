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
    pub engine: Arc<Engine>,
    results: Mutex<Arc<Vec<RuleResult>>>,
    art_results: Mutex<Arc<Vec<RuleResult>>>,
    art_scan: Mutex<Option<ArtifactScan>>,
    plans: Mutex<VecDeque<(String, Arc<Plan>)>>,
    /// A cleanup scan or run is in progress.
    pub cleaning: AtomicBool,
    clean_cancel: Arc<AtomicBool>,
    /// Small UI preferences (last tab, folders, view modes). Kept here, not in the webview, so they
    /// survive the window being released and recreated.
    ui: Mutex<std::collections::HashMap<String, serde_json::Value>>,
    app_icons: Mutex<std::collections::HashMap<String, Option<String>>>,
    app_sizes: Arc<crate::apps::SizeCache>,
    pub history: Arc<crate::history::History>,
    pub tracking: Arc<crate::schedule::Store>,
    pub live: Arc<crate::watch::LiveWatch>,
    /// The window is on screen (live updates every few seconds instead of every 10 minutes).
    pub ui_active: AtomicBool,
}

fn ui_path() -> PathBuf {
    platform::data_dir().join("ui-state.json")
}

fn real_engine() -> Engine {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let reg = Registry::macos().expect("the shipped rules file is validated by tests");
    Engine::new(reg, home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), platform::data_dir().join("undo-log.json"))
}

fn custom_rules_path() -> PathBuf {
    platform::data_dir().join("rules.user.toml")
}

impl AppState {
    pub fn new() -> Self {
        let s = Self::build();
        s.engine.set_custom_rules(&crate::user_rules::load(&custom_rules_path()));
        s
    }

    fn build() -> Self {
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
            history: Arc::new(crate::history::History::load(platform::data_dir().join("history.json"))),
            tracking: Arc::new(crate::schedule::Store::load(platform::data_dir().join("tracking.json"))),
            live: Arc::new(Default::default()),
            ui_active: AtomicBool::new(true),
            app_icons: Mutex::new(Default::default()),
            app_sizes: Arc::new(Default::default()),
            ui: Mutex::new(std::fs::read(ui_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()),
        }
    }

    /// Rules the user switched off in Settings (stored with the other UI preferences).
    pub fn disabled_rules(&self) -> HashSet<String> {
        self.ui.lock().unwrap().get("rules.disabled").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    pub fn ui_get(&self, key: &str) -> Option<serde_json::Value> {
        self.ui.lock().unwrap().get(key).cloned()
    }

    /// Watch the folders of every enabled rule again (rules, locations or switches changed).
    pub fn refresh_watch(&self) {
        let ignore = vec![platform::data_dir(), platform::cache_dir()];
        self.live.rebuild(&self.engine, &self.disabled_rules(), ignore);
    }

    /// Drop everything that can be rebuilt; called when the window (and its webview) goes away.
    pub fn release_memory(&self) {
        self.ui_active.store(false, Relaxed);
        if let Ok(mut m) = self.mon.lock() {
            m.release();
        }
        // The scan tree lives on in the disk cache; keep it in RAM only while it is being looked at.
        if self.running.lock().map(|r| r.is_none()).unwrap_or(true) {
            if let Ok(mut l) = self.loaded.lock() {
                *l = None;
            }
        }
        if let Ok(mut i) = self.app_icons.lock() {
            i.clear();
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
        rules_version: state.engine.registry().rules_version.clone(),
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
    let disabled = state.disabled_rules();
    std::thread::Builder::new()
        .name("cleanup-scan".into())
        .spawn(move || {
            let emit_app = app.clone();
            let res = eng.scan_all(&cancel, &disabled, &move |rr| {
                let _ = emit_app.emit("cleanup-rule", rr);
            });
            let st = app.state::<AppState>();
            if !cancel.load(Relaxed) {
                st.history.record(scan::now_secs(), &res.iter().map(|r| (r.rule.id.clone(), r.total)).collect::<Vec<_>>());
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
    let plan = state.engine.plan_with(&results, &req.ids, &req.picks, req.dry_run, &state.disabled_rules());
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
    let opts = RunOpts { trigger: "manual".into(), confirm_erase: req.confirm_erase, overrides: req.overrides.iter().filter(|o| in_plan.contains(o.as_str())).cloned().collect() };
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
    let disabled = state.disabled_rules();
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
            let (scan, results) = eng.find_artifacts(&roots, &disabled, &cancel, &seen);
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
    ui_put(&state, &key, value);
}

/// Store one UI preference and write the file (also used by the menu bar, which has no webview).
pub fn ui_put(state: &AppState, key: &str, value: serde_json::Value) {
    if key.len() > 64 || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return;
    }
    let key = key.to_string();
    let mut ui = state.ui.lock().unwrap();
    let rules_changed = key == "rules.disabled";
    ui.insert(key, value);
    let path = ui_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(&*ui).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
    drop(ui);
    if rules_changed {
        state.refresh_watch();
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
        rules_version: state.engine.registry().rules_version.clone(),
        rules_count: state.engine.registry().rules.len(),
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

// ───────────────────────────── apps ─────────────────────────────

#[tauri::command(async)]
pub fn apps_list(state: State<'_, AppState>) -> Vec<crate::apps::AppRow> {
    use crate::platform::Apps;
    let me = std::process::id();
    let mut mon = state.mon.lock().unwrap();
    let apps = mon.plat().running_apps();
    let usage = mon.app_usage(&apps);
    let mut icons = state.app_icons.lock().unwrap();
    let rows = apps
        .iter()
        .zip(usage)
        .map(|(a, (mem, cpu, procs))| {
            let icon = icons
                .entry(a.bundle_id.clone())
                .or_insert_with(|| a.bundle_path.as_deref().and_then(|p| mon.plat().app_icon_png(p)).map(|png| format!("data:image/png;base64,{}", crate::apps::base64(&png))))
                .clone();
            let sizes = state.app_sizes.get(&a.bundle_id);
            crate::apps::AppRow {
                pid: a.pid,
                name: a.name.clone(),
                bundle_id: a.bundle_id.clone(),
                path: a.bundle_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                mem,
                cpu,
                procs,
                bundle_bytes: sizes.map(|s| s.0),
                data_bytes: sizes.map(|s| s.1),
                protected: crate::apps::protected_reason(a, me),
                icon,
            }
        })
        .collect();
    drop(icons);
    state.app_sizes.request(&apps, state.engine.home());
    rows
}

#[derive(serde::Deserialize)]
pub struct QuitTarget {
    pub pid: u32,
    pub bundle_id: String,
}

#[derive(Serialize)]
pub struct QuitResult {
    pub pid: u32,
    pub ok: bool,
    pub error: Option<String>,
}

/// Quit or force quit apps. Each target must still be the same running app and not protected.
#[tauri::command(async)]
pub fn apps_quit(state: State<'_, AppState>, targets: Vec<QuitTarget>, force: bool) -> Vec<QuitResult> {
    use crate::platform::Apps;
    let mon = state.mon.lock().unwrap();
    let running = mon.plat().running_apps();
    let me = std::process::id();
    targets
        .iter()
        .map(|t| {
            let res = |ok: bool, e: Option<String>| QuitResult { pid: t.pid, ok, error: e };
            let Some(app) = running.iter().find(|a| a.pid == t.pid && a.bundle_id == t.bundle_id) else {
                return res(false, Some("That app is no longer running.".into()));
            };
            if let Some(why) = crate::apps::protected_reason(app, me) {
                return res(false, Some(format!("{} is protected ({why}).", app.name)));
            }
            if mon.plat().quit_app(app.pid, force) {
                res(true, None)
            } else {
                res(false, Some(format!("{} did not quit.", app.name)))
            }
        })
        .collect()
}

/// Every rule in the registry, for the Settings list.
#[tauri::command(async)]
pub fn rules_list(state: State<'_, AppState>) -> Vec<crate::cleaner::RuleInfo> {
    state.engine.registry().rules.iter().map(Into::into).collect()
}

/// Where each tool location resolved on this Mac, and from which source.
#[tauri::command(async)]
pub fn locations_info(state: State<'_, AppState>) -> Vec<crate::locate::Resolved> {
    let mut v: Vec<_> = state.engine.locations().values().cloned().collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Look for tool folders again (after the user moved a tool or changed a variable).
#[tauri::command(async)]
pub fn locations_redetect(state: State<'_, AppState>) -> Vec<crate::locate::Resolved> {
    state.engine.redetect();
    state.refresh_watch();
    locations_info(state)
}

/// Every rule's status on this Mac (installed, found, location problems), without measuring sizes.
#[tauri::command(async)]
pub fn rules_status(state: State<'_, AppState>) -> Vec<crate::cleaner::RuleStatus> {
    state.engine.status_all()
}

// ───────────────────────────── custom rules ─────────────────────────────

use crate::user_rules::{self as ur, Draft};

#[tauri::command(async)]
pub fn rules_custom() -> Vec<Draft> {
    ur::load(&custom_rules_path())
}

/// Try a draft: validation plus what it would match right now. Changes nothing.
#[tauri::command(async)]
pub fn rule_test(state: State<'_, AppState>, draft: Draft) -> crate::cleaner::RuleTest {
    state.engine.test_rule(&draft)
}

/// Save a custom rule (new or edited). Returns its id.
#[tauri::command(async)]
pub fn rule_save(state: State<'_, AppState>, mut draft: Draft) -> Result<String, String> {
    let t = state.engine.test_rule(&draft);
    if !t.ok {
        return Err(t.error.unwrap_or_else(|| "This rule can't be saved.".into()));
    }
    let path = custom_rules_path();
    let mut all = ur::load(&path);
    match all.iter().position(|d| !draft.id.is_empty() && d.id == draft.id) {
        Some(i) => all[i] = draft.clone(),
        None => {
            let taken: HashSet<String> = state.engine.registry().rules.iter().map(|r| r.id.clone()).collect();
            draft.id = ur::new_id(&draft.name, &taken);
            all.push(draft.clone());
        }
    }
    ur::save(&path, &all).map_err(|e| format!("Could not save the rule: {e}"))?;
    state.engine.set_custom_rules(&all);
    state.refresh_watch();
    Ok(draft.id)
}

#[tauri::command(async)]
pub fn rule_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let path = custom_rules_path();
    let mut all = ur::load(&path);
    all.retain(|d| d.id != id);
    ur::save(&path, &all).map_err(|e| e.to_string())?;
    state.engine.set_custom_rules(&all);
    state.refresh_watch();
    Ok(())
}

#[tauri::command(async)]
pub fn rules_export(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let Some(dest) = state.mon.lock().unwrap().plat().pick_save_file("deskveet-rules.toml") else { return Ok(None) };
    std::fs::write(&dest, ur::to_toml(&ur::load(&custom_rules_path()))).map_err(|e| e.to_string())?;
    Ok(Some(dest.to_string_lossy().into_owned()))
}

#[derive(Serialize)]
pub struct ImportResult {
    pub added: usize,
    pub skipped: Vec<String>,
}

/// Import custom rules from a file. Each rule is validated and tested like one typed in by hand.
#[tauri::command(async)]
pub fn rules_import(state: State<'_, AppState>) -> Result<Option<ImportResult>, String> {
    let Some(src) = state.mon.lock().unwrap().plat().pick_file() else { return Ok(None) };
    let text = std::fs::read_to_string(&src).map_err(|e| e.to_string())?;
    let incoming = ur::parse(&text)?;
    let path = custom_rules_path();
    let mut all = ur::load(&path);
    let mut taken: HashSet<String> = state.engine.registry().rules.iter().map(|r| r.id.clone()).collect();
    let (mut added, mut skipped) = (0, Vec::new());
    for mut d in incoming {
        let t = state.engine.test_rule(&d);
        if !t.ok {
            skipped.push(format!("{}: {}", d.name, t.error.unwrap_or_default()));
            continue;
        }
        d.id = ur::new_id(&d.name, &taken);
        taken.insert(d.id.clone());
        all.push(d);
        added += 1;
    }
    ur::save(&path, &all).map_err(|e| e.to_string())?;
    state.engine.set_custom_rules(&all);
    Ok(Some(ImportResult { added, skipped }))
}

// ───────────────────────────── tracking: history, alerts, schedules ─────────────────────────────

use crate::history::{growth_per_day, Point, DAY};
use crate::schedule::{is_due, Schedule};

#[derive(Serialize)]
pub struct TrackInfo {
    /// Up to 60 points for a sparkline.
    pub points: Vec<Point>,
    pub growth_per_day: Option<f64>,
    pub last_cleaned: Option<u64>,
    pub freed: u64,
    pub alert: Option<u64>,
    pub schedule: Option<Schedule>,
    /// Why it can't be scheduled (with review consent), if it can't.
    pub schedule_problem: Option<String>,
}

#[derive(Serialize)]
pub struct Tracking {
    pub rules: std::collections::HashMap<String, TrackInfo>,
    pub disk_free_pct: Option<f32>,
}

fn sample(v: &[Point], n: usize) -> Vec<Point> {
    if v.len() <= n {
        return v.to_vec();
    }
    (0..n).map(|i| v[i * (v.len() - 1) / (n - 1)]).collect()
}

#[tauri::command(async)]
pub fn tracking_get(state: State<'_, AppState>) -> Tracking {
    let now = scan::now_secs();
    let cleaned = state.engine.undo.summary();
    let cfg = state.tracking.cfg.lock().unwrap().clone();
    let rules = state
        .engine
        .registry()
        .rules
        .iter()
        .map(|r| {
            let pts = state.history.get(&r.id);
            let c = cleaned.get(&r.id);
            (r.id.clone(), TrackInfo {
                growth_per_day: growth_per_day(&pts, now, 30 * DAY),
                points: sample(&pts, 60),
                last_cleaned: c.map(|c| c.0),
                freed: c.map(|c| c.1).unwrap_or(0),
                alert: cfg.alerts.get(&r.id).copied(),
                schedule: cfg.schedules.get(&r.id).copied(),
                schedule_problem: state.engine.schedule_problem(&r.id, true),
            })
        })
        .collect();
    Tracking { rules, disk_free_pct: cfg.disk_free_pct }
}

#[tauri::command(async)]
pub fn alert_set(state: State<'_, AppState>, rule_id: String, bytes: Option<u64>) {
    let mut c = state.tracking.cfg.lock().unwrap();
    match bytes.filter(|b| *b > 0) {
        Some(b) => c.alerts.insert(rule_id, b),
        None => c.alerts.remove(&rule_id),
    };
    drop(c);
    state.tracking.save();
}

#[tauri::command(async)]
pub fn disk_alert_set(state: State<'_, AppState>, pct: Option<f32>) {
    state.tracking.cfg.lock().unwrap().disk_free_pct = pct.filter(|p| *p > 0.0 && *p < 100.0);
    state.tracking.save();
}

#[tauri::command(async)]
pub fn schedule_set(state: State<'_, AppState>, rule_id: String, schedule: Option<Schedule>) -> Result<(), String> {
    if let Some(s) = &schedule {
        if let Some(why) = state.engine.schedule_problem(&rule_id, s.review_ok) {
            return Err(why);
        }
    }
    let mut c = state.tracking.cfg.lock().unwrap();
    match schedule {
        Some(mut s) => {
            s.hour = s.hour.min(23);
            s.weekday = s.weekday.min(6);
            s.created = scan::now_secs();
            c.schedules.insert(rule_id, s);
        }
        None => {
            c.schedules.remove(&rule_id);
        }
    }
    drop(c);
    state.tracking.save();
    Ok(())
}

/// Sends a sample notification. On failure (usually notifications turned off for the app) it opens the
/// Notifications page in System Settings and returns the reason.
#[tauri::command(async)]
pub fn notify_test() -> Result<(), String> {
    let plat = crate::platform::current();
    use crate::platform::Platform;
    plat.notify_checked("Deskveet", "Notifications are working. You'll see alerts and schedule summaries like this.").inspect_err(|_| {
        let _ = plat.open_notification_settings();
    })
}

fn bytes_h(b: u64) -> String {
    let (v, u) = if b >= 1_000_000_000 { (b as f64 / 1e9, "GB") } else { (b as f64 / 1e6, "MB") };
    format!("{v:.1} {u}")
}

/// Runs for the life of the process (also with the window closed): due schedules every minute, alerts every
/// 15 minutes. Sleeps the rest of the time; nothing is kept in memory between checks.
pub fn start_tracker(app: AppHandle) {
    const TICK: Duration = Duration::from_secs(60);
    const ALERT_EVERY: u64 = 15 * 60;
    let _ = std::thread::Builder::new().name("tracker".into()).spawn(move || {
        platform::background_thread();
        let mut last_alert_check = 0u64;
        loop {
            std::thread::sleep(TICK);
            let st = app.state::<AppState>();
            let now = scan::now_secs();
            run_due_schedules(&app, &st, now);
            crate::tray::refresh(&app);
            if now.saturating_sub(last_alert_check) >= ALERT_EVERY {
                last_alert_check = now;
                check_alerts(&st, now);
            }
        }
    });
}

fn run_due_schedules(app: &AppHandle, st: &AppState, now: u64) {
    let due: Vec<(String, Schedule)> = {
        let c = st.tracking.cfg.lock().unwrap();
        c.schedules.iter().filter(|(id, s)| is_due(s, c.last_run.get(*id).copied(), s.created, now)).map(|(id, s)| (id.clone(), *s)).collect()
    };
    for (id, s) in due {
        if st.cleaning.swap(true, Relaxed) {
            return; // a person is scanning or cleaning: try again next minute
        }
        let res = st.engine.run_scheduled(&id, s.review_ok, &st.disabled_rules());
        st.cleaning.store(false, Relaxed);
        st.tracking.cfg.lock().unwrap().last_run.insert(id.clone(), now);
        st.tracking.save();
        let name = st.engine.registry().get(&id).map(|r| r.name.clone()).unwrap_or(id.clone());
        match res {
            Ok(rep) if rep.ok > 0 => {
                let mut body = format!("Moved {} item{} ({}) to the Trash.", rep.ok, if rep.ok == 1 { "" } else { "s" }, bytes_h(rep.trashed_bytes));
                if rep.skipped > 0 {
                    body.push_str(&format!(" {} in use were left alone.", rep.skipped));
                }
                st.mon.lock().unwrap().plat().notify(&format!("Scheduled clean: {name}"), &body);
                *st.results.lock().unwrap() = Arc::new(Vec::new());
                let _ = app.emit("tracking-updated", ());
            }
            Ok(_) => {}
            Err(e) => st.mon.lock().unwrap().plat().notify(&format!("Scheduled clean skipped: {name}"), &e),
        }
    }
}

fn check_alerts(st: &AppState, now: u64) {
    let cfg = st.tracking.cfg.lock().unwrap().clone();
    let recently = |key: &str| cfg.last_alert.get(key).is_some_and(|t| now.saturating_sub(*t) < DAY);
    let mut fired: Vec<String> = Vec::new();
    // disk
    if let Some(pct) = cfg.disk_free_pct {
        let home = st.engine.home().to_path_buf();
        let free = crate::fsutil::free_bytes(&home);
        let total = crate::sys::volume_total(&home);
        if total > 0 && (free as f64 / total as f64 * 100.0) < pct as f64 && !recently("disk") {
            st.mon.lock().unwrap().plat().notify("Your disk is almost full", &format!("{} free ({:.0}%). Open Deskveet to see what you can clean.", bytes_h(free), free as f64 / total as f64 * 100.0));
            fired.push("disk".into());
        }
    }
    // rules over their size alert (measured now, which also feeds the history)
    if !cfg.alerts.is_empty() {
        let procs: Vec<crate::sys::ProcInfo> = Vec::new();
        let reg = st.engine.registry();
        let mut sizes = Vec::new();
        for (id, limit) in &cfg.alerts {
            let Some(rule) = reg.get(id) else { continue };
            let total = st.engine.scan_rule(rule, &procs).total;
            sizes.push((id.clone(), total));
            if total >= *limit && !recently(id) {
                st.mon.lock().unwrap().plat().notify(&format!("{} is {}", rule.name, bytes_h(total)), &format!("That's over the {} alert you set. Open Deskveet to clean it.", bytes_h(*limit)));
                fired.push(id.clone());
            }
        }
        st.history.record(now, &sizes);
    }
    if !fired.is_empty() {
        let mut c = st.tracking.cfg.lock().unwrap();
        for k in fired {
            c.last_alert.insert(k, now);
        }
        drop(c);
        st.tracking.save();
    }
}

// ───────────────────────────── live updates and background mode ─────────────────────────────

#[tauri::command(async)]
pub fn ui_visible(state: State<'_, AppState>, visible: bool) {
    state.ui_active.store(visible, Relaxed);
}

/// Re-measure rules whose folders changed: every few seconds while the window is visible, at most every
/// 10 minutes per rule otherwise. Results go to the open window, the history and the size alerts.
pub fn start_live(app: AppHandle) {
    let _ = std::thread::Builder::new().name("live".into()).spawn(move || {
        platform::background_thread();
        app.state::<AppState>().refresh_watch();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let st = app.state::<AppState>();
            let active = st.ui_active.load(Relaxed);
            let due = st.live.take_due(if active { Duration::from_secs(5) } else { Duration::from_secs(600) });
            if due.is_empty() || st.cleaning.load(Relaxed) {
                continue;
            }
            let reg = st.engine.registry();
            let procs = crate::sys::RealSys::new(st.engine.home().to_path_buf()).processes_light();
            let now = scan::now_secs();
            let mut sizes = Vec::new();
            for id in due {
                let Some(rule) = reg.get(&id) else { continue };
                let rr = st.engine.scan_rule(rule, &procs);
                sizes.push((id.clone(), rr.total));
                {
                    let mut res = st.results.lock().unwrap();
                    if let Some(i) = res.iter().position(|r| r.rule.id == id) {
                        let mut v = res.as_ref().clone();
                        v[i] = rr.clone();
                        *res = Arc::new(v);
                    }
                }
                if active {
                    let _ = app.emit("rule-updated", &rr);
                }
                let limit = st.tracking.cfg.lock().unwrap().alerts.get(&id).copied();
                if let Some(limit) = limit {
                    let last = st.tracking.cfg.lock().unwrap().last_alert.get(&id).copied().unwrap_or(0);
                    if rr.total >= limit && now.saturating_sub(last) >= DAY {
                        st.mon.lock().unwrap().plat().notify(&format!("{} is {}", rule.name, bytes_h(rr.total)), &format!("That's over the {} alert you set. Open Deskveet to clean it.", bytes_h(limit)));
                        st.tracking.cfg.lock().unwrap().last_alert.insert(id.clone(), now);
                        st.tracking.save();
                    }
                }
            }
            st.history.record(now, &sizes);
        }
    });
}

#[derive(Serialize)]
pub struct Background {
    pub menubar: bool,
    pub login: bool,
    /// macOS needs the user to approve the login item in System Settings.
    pub login_needs_approval: bool,
    pub watched_rules: usize,
}

fn menubar_on(state: &AppState) -> bool {
    state.ui.lock().unwrap().get("bg.menubar").and_then(|v| v.as_bool()).unwrap_or(false)
}

fn login_status() -> (bool, bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2_service_management::{SMAppService, SMAppServiceStatus};
        // SAFETY: plain property reads on the shared service object.
        let st = unsafe { SMAppService::mainAppService().status() };
        (st == SMAppServiceStatus::Enabled || st == SMAppServiceStatus::RequiresApproval, st == SMAppServiceStatus::RequiresApproval)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (false, false)
    }
}

#[tauri::command(async)]
pub fn background_get(state: State<'_, AppState>) -> Background {
    let (login, approval) = login_status();
    Background { menubar: menubar_on(&state), login, login_needs_approval: approval, watched_rules: state.live.watched_rules() }
}

#[tauri::command]
pub fn background_set(app: AppHandle, state: State<'_, AppState>, menubar: Option<bool>, login: Option<bool>) -> Result<Background, String> {
    if let Some(on) = menubar {
        state.ui.lock().unwrap().insert("bg.menubar".into(), serde_json::Value::Bool(on));
        if on {
            crate::tray::show(&app).map_err(|e| e.to_string())?;
        } else {
            crate::tray::hide(&app);
        }
    }
    if let Some(on) = login {
        #[cfg(target_os = "macos")]
        {
            use objc2_service_management::SMAppService;
            // SAFETY: registering the app itself as a login item; errors are surfaced to the user.
            let r = unsafe { if on { SMAppService::mainAppService().registerAndReturnError() } else { SMAppService::mainAppService().unregisterAndReturnError() } };
            r.map_err(|e| format!("macOS refused: {}", e.localizedDescription()))?;
        }
    }
    let (login, approval) = login_status();
    Ok(Background { menubar: menubar_on(&state), login, login_needs_approval: approval, watched_rules: state.live.watched_rules() })
}

