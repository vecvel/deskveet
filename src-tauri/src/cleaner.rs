//! Cleanup engine: rules -> items (scan) -> verified preview (plan) -> Trash / tool / erase (run).
//!
//! The UI never supplies paths for rule-based cleaning, only item ids that the engine itself produced,
//! and every path is re-checked by `Safety` when the plan is built and again right before it is acted on.

use crate::fsutil::{self, Measure};
use crate::locate::{self, Locations, Resolved};
use crate::rules::{Kind, Method, Registry, Risk, Rule};
use crate::safety::{Allowed, ArtifactMatcher, RuleMatcher, Safety};
use crate::sys::{ProcInfo, Sys};
use crate::undo::{Entry, UndoLog};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

// ───────────────────────────── data model ─────────────────────────────

#[derive(Serialize, Clone, Debug)]
pub struct RuleInfo {
    pub id: String,
    pub name: String,
    pub category: String,
    pub kind: Kind,
    pub risk: Risk,
    pub method: Method,
    pub description: String,
    pub regenerates: String,
    pub tool: Option<String>,
    pub opt_in: bool,
    pub needs_admin: bool,
    pub custom: bool,
    /// The rule's patterns as written (with `{location}` tokens), for duplicating and display.
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
}

impl From<&Rule> for RuleInfo {
    fn from(r: &Rule) -> Self {
        RuleInfo {
            id: r.id.clone(),
            name: r.name.clone(),
            category: r.category.clone(),
            kind: r.kind,
            risk: r.risk,
            method: r.method,
            description: r.description.clone(),
            regenerates: r.regenerates.clone(),
            tool: r.tool_display(),
            opt_in: r.opt_in,
            needs_admin: r.needs_admin,
            custom: r.custom,
            paths: r.paths.clone(),
            exclude: r.exclude.clone(),
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Item {
    pub id: String,
    pub rule_id: String,
    pub label: String,
    pub path: Option<String>,
    pub size: u64,
    pub files: u64,
    pub mtime: i64,
    pub blocked: Option<String>,
    pub info: Vec<String>,
    /// udid / runtime id for tool placeholders. Never sent to the UI.
    #[serde(skip)]
    pub extra: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RuleResult {
    pub rule: RuleInfo,
    /// Where this rule looked on this Mac (only for rules that use `{location}` tokens).
    pub locations: Vec<Resolved>,
    pub available: bool,
    pub note: Option<String>,
    pub items: Vec<Item>,
    pub total: u64,
    /// Processes from the rule's `busy` list that are running right now.
    pub running: Vec<String>,
}

/// Result of trying a custom rule before saving it.
#[derive(Serialize, Clone, Debug)]
pub struct RuleTest {
    pub ok: bool,
    pub error: Option<String>,
    pub count: usize,
    /// Bytes of the largest matches that were measured (`measured` of them).
    pub total: u64,
    pub measured: usize,
    pub items: Vec<TestItem>,
    pub blocked: Vec<TestItem>,
}

#[derive(Serialize, Clone, Debug)]
pub struct TestItem {
    pub path: String,
    pub size: u64,
    pub why: Option<String>,
}

/// What a rule can do on this Mac, without measuring anything (fast enough for a settings list).
#[derive(Serialize, Clone, Debug)]
pub struct RuleStatus {
    pub id: String,
    /// ready | not_installed | nothing_found | location_ignored | needs_fda | on_demand
    pub state: &'static str,
    pub note: Option<String>,
    pub locations: Vec<Resolved>,
    /// Folders or files matched right now (capped).
    pub matches: usize,
}

#[derive(Clone, Debug)]
enum Source {
    Rule(String),
    Pick,
}

#[derive(Serialize, Clone, Debug)]
pub struct PlanItem {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub label: String,
    pub path: Option<String>,
    pub size: u64,
    pub method: Method,
    pub risk: Risk,
    pub command: Option<String>,
    /// Can the user get it back? (Trash: yes. simctl delete / erase: no.)
    pub recoverable: bool,
    /// Hard stop: cannot be cleaned.
    pub blocked: Option<String>,
    /// Soft stops: each must be acknowledged by the user in the preview.
    pub confirm: Vec<String>,
    pub admin_cmd: Option<String>,
    #[serde(skip)]
    real: Option<PathBuf>,
    #[serde(skip)]
    argv: Option<Vec<String>>,
    #[serde(skip)]
    source: Option<Source>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Plan {
    pub token: String,
    pub dry_run: bool,
    pub items: Vec<PlanItem>,
    pub total: u64,
    /// At least one item permanently erases data: needs a second confirmation.
    pub erase: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct ItemResult {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    /// ok | failed | skipped | dry-run
    pub status: String,
    pub method: Method,
    pub bytes: u64,
    pub trash_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Report {
    pub run: String,
    pub dry_run: bool,
    pub results: Vec<ItemResult>,
    /// Moved to the Trash: disk space comes back when the Trash is emptied.
    pub trashed_bytes: u64,
    /// Freed right away (tools and erase).
    pub freed_bytes: u64,
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
    pub undo_log: String,
}

#[derive(Default)]
pub struct RunOpts {
    /// "manual" (default) or "scheduled"; recorded in the undo log.
    pub trigger: String,
    pub confirm_erase: bool,
    /// Item ids whose soft stops the user acknowledged.
    pub overrides: HashSet<String>,
}

// ───────────────────────────── engine ─────────────────────────────

pub struct Engine {
    /// Built-in rules plus the user's custom rules; replaced when custom rules change.
    reg: std::sync::RwLock<Arc<Registry>>,
    builtin: Arc<Registry>,
    pub safety: Safety,
    pub sys: Arc<dyn Sys>,
    pub trash_dir: PathBuf,
    pub undo: UndoLog,
    /// Folders the user chose for the artifact finder; artifact items are only valid below these.
    art_roots: Mutex<Vec<PathBuf>>,
    locs: Mutex<Option<Arc<Locations>>>,
    me: u32,
}

/// Whichever matcher a rule needs.
enum Guard {
    Glob(RuleMatcher),
    Art(ArtifactMatcher),
}

impl Guard {
    fn allowed(&self) -> Allowed<'_> {
        match self {
            Guard::Glob(m) => Allowed::Rule(m),
            Guard::Art(m) => Allowed::Artifact(m),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct ArtItem {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub dir: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub blocked: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Project {
    pub path: String,
    pub name: String,
    /// Newest change among the project's own files (artifact folders excluded), unix seconds.
    pub last_active: i64,
    pub items: Vec<ArtItem>,
    pub total: u64,
    pub running: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ArtifactScan {
    pub roots: Vec<String>,
    pub projects: Vec<Project>,
    pub total: u64,
    pub dirs: u64,
}

fn sv<const N: usize>(a: [&str; N]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn pretty_runtime(key: &str) -> String {
    let k = key.rsplit('.').next().unwrap_or(key);
    match k.split_once('-') {
        Some((os, ver)) => format!("{os} {}", ver.replace('-', ".")),
        None => k.to_string(),
    }
}

fn valid_udid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn running_matches(procs: &[ProcInfo], tokens: &[String], me: u32) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| procs.iter().any(|p| p.pid != me && (p.name.eq_ignore_ascii_case(t) || (t.len() >= 8 && p.cmd.contains(t.as_str())))))
        .cloned()
        .collect()
}

/// Directories never descended into while looking for projects.
const SKIP_DIRS: &[&str] = &[".git", "Library", ".Trash", "node_modules", ".npm", ".cache", "Applications", ".cargo", ".rustup", ".pub-cache", ".gradle", ".android", ".nvm", ".docker"];
const MAX_DEPTH: usize = 9;

fn walk_artifacts(dir: &Path, depth: usize, rules: &[&Rule], cancel: &AtomicBool, seen: &std::sync::atomic::AtomicU64) -> Vec<(usize, PathBuf)> {
    if cancel.load(Relaxed) || depth > MAX_DEPTH {
        return Vec::new();
    }
    seen.fetch_add(1, Relaxed);
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut found = Vec::new();
    let mut descend = Vec::new();
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() {
            continue; // symlinks are not followed
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let mut matched = false;
        for (i, r) in rules.iter().enumerate() {
            if r.dir_name.as_deref() == Some(name.as_str()) && (r.markers.is_empty() || r.markers.iter().any(|m| dir.join(m).exists())) {
                found.push((i, e.path()));
                matched = true;
                break; // one rule per folder
            }
        }
        if !matched && !SKIP_DIRS.contains(&name.as_str()) {
            descend.push(e.path());
        }
    }
    found.extend(descend.par_iter().flat_map(|d| walk_artifacts(d, depth + 1, rules, cancel, seen)).collect::<Vec<_>>());
    found
}

/// Newest change among the project's files, ignoring the artifact folders themselves.
fn project_activity(project: &Path, artifact_dirs: &[&str]) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(rd) = fs::read_dir(project) else { return 0 };
    let mut newest = 0;
    for e in rd.flatten().take(400) {
        if artifact_dirs.contains(&e.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        if let Ok(m) = e.metadata() {
            newest = newest.max(m.mtime());
        }
    }
    if newest == 0 {
        newest = fs::metadata(project).map(|m| m.mtime()).unwrap_or(0);
    }
    newest
}

fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0));
    format!("{:016x}", h.finish())
}

impl Engine {
    pub fn new(reg: Registry, home: PathBuf, sys: Arc<dyn Sys>, trash_dir: PathBuf, undo_path: PathBuf) -> Engine {
        let reg = Arc::new(reg);
        Engine { reg: std::sync::RwLock::new(reg.clone()), builtin: reg, safety: Safety::new(home), sys, trash_dir, undo: UndoLog::new(undo_path), art_roots: Mutex::new(Vec::new()), locs: Mutex::new(None), me: std::process::id() }
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.reg.read().unwrap().clone()
    }

    pub fn builtin(&self) -> Arc<Registry> {
        self.builtin.clone()
    }

    /// Use these custom rules from now on (built-in rules stay as they are).
    pub fn set_custom_rules(&self, drafts: &[crate::user_rules::Draft]) {
        let mut merged = Registry { schema_version: self.builtin.schema_version, rules_version: self.builtin.rules_version.clone(), locations: self.builtin.locations.clone(), rules: self.builtin.rules.clone() };
        for d in drafts {
            if merged.get(&d.id).is_none() {
                merged.rules.push(d.to_rule());
            }
        }
        *self.reg.write().unwrap() = Arc::new(merged);
    }

    pub fn home(&self) -> &Path {
        self.safety.home()
    }

    fn pool<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        match rayon::ThreadPoolBuilder::new().num_threads(4).thread_name(|i| format!("clean-{i}")).build() {
            Ok(p) => p.install(f),
            Err(_) => f(),
        }
    }

    /// Tool locations on this Mac, resolved once (tool query, then environment, then default).
    pub fn locations(&self) -> Arc<Locations> {
        let mut g = self.locs.lock().unwrap();
        if let Some(l) = g.as_ref() {
            return l.clone();
        }
        let env = self.sys.env();
        let sys = self.sys.clone();
        let l = Arc::new(locate::resolve(&self.registry().locations, &env, &move |argv: &[String]| sys.query(argv), &self.safety));
        *g = Some(l.clone());
        l
    }

    /// Forget resolved locations (the user moved a tool or changed a variable).
    pub fn redetect(&self) {
        *self.locs.lock().unwrap() = None;
    }

    /// The rule's include and exclude globs with `{location}` tokens and `~` replaced. Patterns whose
    /// location could not be found are dropped.
    fn expanded(&self, rule: &Rule) -> (Vec<String>, Vec<String>) {
        let locs = self.locations();
        let ex = |v: &[String]| v.iter().filter_map(|p| locate::expand(p, &locs, self.home())).collect();
        (ex(&rule.paths), ex(&rule.exclude))
    }

    fn rule_locations(&self, rule: &Rule) -> Vec<Resolved> {
        let locs = self.locations();
        let mut ids: Vec<&str> = rule.paths.iter().chain(&rule.exclude).flat_map(|p| locate::tokens(p)).collect();
        ids.dedup();
        ids.iter().filter_map(|id| locs.get(*id).cloned()).collect()
    }

    fn matcher(&self, rule: &Rule) -> RuleMatcher {
        let (inc, exc) = self.expanded(rule);
        RuleMatcher::from_expanded(&inc, &exc)
    }

    fn guard(&self, rule: &Rule) -> Guard {
        if rule.kind == Kind::Artifact {
            Guard::Art(ArtifactMatcher::new(rule, &self.art_roots.lock().unwrap()))
        } else {
            Guard::Glob(self.matcher(rule))
        }
    }

    fn rule_paths(&self, rule: &Rule) -> Vec<PathBuf> {
        let (inc, exc) = self.expanded(rule);
        fsutil::expand(&inc, &exc, self.home())
    }

    // ── scanning ──

    pub fn scan_all(&self, cancel: &AtomicBool, disabled: &HashSet<String>, on_rule: &(dyn Fn(&RuleResult) + Sync)) -> Vec<RuleResult> {
        let procs = self.sys.processes();
        let reg = self.registry();
        let out: Vec<Option<RuleResult>> = self.pool(|| {
            reg
                .rules
                .par_iter()
                .filter(|r| r.kind != Kind::Artifact && !disabled.contains(&r.id))
                .map(|r| {
                    if cancel.load(Relaxed) {
                        return None;
                    }
                    let rr = self.scan_rule(r, &procs);
                    on_rule(&rr);
                    Some(rr)
                })
                .collect()
        });
        out.into_iter().flatten().collect()
    }

    pub fn scan_rule(&self, rule: &Rule, procs: &[ProcInfo]) -> RuleResult {
        let mut rr = RuleResult { rule: rule.into(), locations: self.rule_locations(rule), available: true, note: None, items: Vec::new(), total: 0, running: running_matches(procs, &rule.busy, self.me) };
        if let Some(why) = self.not_installed(rule) {
            rr.available = false;
            rr.note = Some(why);
            return rr;
        }
        let items = match (rule.kind, rule.dynamic.as_deref()) {
            (Kind::Dynamic, Some("simctl_devices")) => self.sim_devices(rule),
            (Kind::Dynamic, Some("simctl_runtimes")) => self.sim_runtimes(rule),
            (Kind::Dynamic, Some("trash")) => self.trash_items(rule),
            _ if rule.id == "simulators.unavailable" => self.sim_unavailable(rule),
            _ if rule.method == Method::Tool => Ok(self.tool_items(rule)),
            _ => Ok(self.path_items(rule)),
        };
        match items {
            Ok(mut v) => {
                v.sort_by(|a, b| b.size.cmp(&a.size));
                rr.total = v.iter().filter(|i| i.blocked.is_none()).map(|i| i.size).sum();
                rr.items = v;
            }
            Err(e) => rr.note = Some(e),
        }
        rr
    }

    fn new_item(&self, rule: &Rule, key: &str, label: String) -> Item {
        Item { id: format!("{}|{}", rule.id, key), rule_id: rule.id.clone(), label, ..Default::default() }
    }

    fn path_items(&self, rule: &Rule) -> Vec<Item> {
        let m = self.matcher(rule);
        let paths = self.rule_paths(rule);
        // `…/Chrome/*/Cache` matches many folders literally called "Cache": show "<profile>/Cache".
        let two_part = rule.paths.iter().any(|p| p.rsplit_once('/').is_some_and(|(head, _)| head.contains('*')));
        let measured: Vec<(PathBuf, Measure)> = paths.into_par_iter().map(|p| { let me = fsutil::measure(&p); (p, me) }).collect();
        measured
            .into_iter()
            .filter(|(_, me)| me.bytes > 0 || me.files > 0)
            .map(|(p, me)| {
                let leaf = |q: &Path| q.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let name = match (two_part, p.parent()) {
                    (true, Some(par)) => format!("{}/{}", leaf(par), leaf(&p)),
                    _ => leaf(&p),
                };
                let mut it = self.new_item(rule, &p.to_string_lossy(), name);
                it.path = Some(p.to_string_lossy().into_owned());
                it.size = me.bytes;
                it.files = me.files;
                it.mtime = me.mtime;
                if let Err(d) = self.safety.check(&p, &Allowed::Rule(&m)) {
                    it.blocked = Some(d.to_string());
                }
                it
            })
            .collect()
    }

    /// One aggregate item: the tool cleans its whole cache, not individual folders.
    fn tool_items(&self, rule: &Rule) -> Vec<Item> {
        let paths = self.rule_paths(rule);
        let me = paths.par_iter().map(|p| fsutil::measure(p)).reduce(Measure::default, |a, b| Measure { bytes: a.bytes + b.bytes, files: a.files + b.files, mtime: a.mtime.max(b.mtime) });
        if !rule.paths.is_empty() && me.files == 0 && me.bytes == 0 {
            return Vec::new();
        }
        let mut it = self.new_item(rule, "tool", rule.name.clone());
        it.size = me.bytes;
        it.files = me.files;
        it.mtime = me.mtime;
        it.info.push(format!("Runs: {}", rule.tool_display().unwrap_or_default()));
        vec![it]
    }

    fn trash_items(&self, rule: &Rule) -> Result<Vec<Item>, String> {
        // macOS hides the Trash from apps without Full Disk Access: say so instead of showing "empty".
        if let Err(e) = fs::read_dir(&self.trash_dir) {
            if matches!(e.kind(), std::io::ErrorKind::PermissionDenied) || e.raw_os_error() == Some(1) {
                return Err("Full Disk Access is needed to see and empty the Trash".into());
            }
        }
        let me = fsutil::measure(&self.trash_dir);
        if me.files == 0 {
            return Ok(Vec::new());
        }
        let mut it = self.new_item(rule, "trash", "Trash".into());
        it.path = Some(self.trash_dir.to_string_lossy().into_owned());
        it.size = me.bytes;
        it.files = me.files;
        it.mtime = me.mtime;
        it.info.push("Permanently deletes everything in the Trash".into());
        Ok(vec![it])
    }

    fn simctl_json(&self, argv: Vec<String>) -> Result<Value, String> {
        let out = self.sys.capture(&argv)?;
        serde_json::from_str(&out).map_err(|e| format!("could not read simctl output: {e}"))
    }

    fn device_dir(&self, udid: &str) -> PathBuf {
        self.home().join("Library/Developer/CoreSimulator/Devices").join(udid)
    }

    fn sim_devices(&self, rule: &Rule) -> Result<Vec<Item>, String> {
        let v = self.simctl_json(sv(["xcrun", "simctl", "list", "devices", "-j"]))?;
        let mut raw = Vec::new();
        for (key, arr) in v["devices"].as_object().into_iter().flatten() {
            for d in arr.as_array().into_iter().flatten() {
                if let Some(udid) = d["udid"].as_str().filter(|u| valid_udid(u)) {
                    raw.push((key.clone(), udid.to_string(), d.clone()));
                }
            }
        }
        let m = self.matcher(rule);
        Ok(raw
            .into_par_iter()
            .map(|(key, udid, d)| {
                let dir = self.device_dir(&udid);
                let me = if dir.is_dir() { fsutil::measure(&dir) } else { Measure { bytes: d["dataPathSize"].as_u64().unwrap_or(0), ..Default::default() } };
                let mut it = self.new_item(rule, &udid, format!("{} · {}", d["name"].as_str().unwrap_or("Simulator"), pretty_runtime(&key)));
                it.extra = Some(udid.clone());
                if dir.is_dir() {
                    it.path = Some(dir.to_string_lossy().into_owned());
                    if let Err(e) = self.safety.check(&dir, &Allowed::Rule(&m)) {
                        it.blocked = Some(e.to_string());
                    }
                }
                it.size = me.bytes;
                it.files = me.files;
                let state = d["state"].as_str().unwrap_or("Unknown");
                it.info.push(state.to_string());
                it.info.push(match d["lastBootedAt"].as_str() {
                    Some(t) => format!("last booted {}", t.get(..10).unwrap_or(t)),
                    None => "never booted".into(),
                });
                if d["isAvailable"].as_bool() == Some(false) {
                    it.info.push("unavailable (runtime missing)".into());
                }
                if state != "Shutdown" && it.blocked.is_none() {
                    it.blocked = Some(format!("{state}: shut it down in Simulator first"));
                }
                it
            })
            .collect())
    }

    fn sim_runtimes(&self, rule: &Rule) -> Result<Vec<Item>, String> {
        let rt = self.simctl_json(sv(["xcrun", "simctl", "runtime", "list", "-j"]))?;
        let devices = self.simctl_json(sv(["xcrun", "simctl", "list", "devices", "-j"])).ok();
        let mut out = Vec::new();
        for (_, r) in rt.as_object().into_iter().flatten() {
            let Some(id) = r["identifier"].as_str().filter(|i| valid_id(i)) else { continue };
            let rid = r["runtimeIdentifier"].as_str().unwrap_or("");
            let mut it = self.new_item(rule, id, format!("{} ({})", pretty_runtime(rid), r["build"].as_str().unwrap_or("?")));
            it.extra = Some(id.to_string());
            it.size = r["sizeBytes"].as_u64().unwrap_or(0);
            let users = devices.as_ref().map(|d| d["devices"][rid].as_array().map_or(0, |a| a.len())).unwrap_or(0);
            it.info.push(if users == 0 { "no simulator devices use it".to_string() } else { format!("{users} simulator device(s) use it and become unavailable") });
            it.info.push(r["state"].as_str().unwrap_or("").to_string());
            if r["deletable"].as_bool() == Some(false) {
                it.blocked = Some("Built into Xcode and cannot be deleted".into());
            }
            out.push(it);
        }
        Ok(out)
    }

    fn sim_unavailable(&self, rule: &Rule) -> Result<Vec<Item>, String> {
        let v = self.simctl_json(sv(["xcrun", "simctl", "list", "devices", "-j"]))?;
        let (mut n, mut bytes) = (0u64, 0u64);
        for (_, arr) in v["devices"].as_object().into_iter().flatten() {
            for d in arr.as_array().into_iter().flatten() {
                if d["isAvailable"].as_bool() == Some(false) {
                    n += 1;
                    bytes += d["dataPathSize"].as_u64().unwrap_or(0);
                }
            }
        }
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut it = self.new_item(rule, "tool", rule.name.clone());
        it.size = bytes;
        it.info.push(format!("{n} unavailable device(s)"));
        it.info.push(format!("Runs: {}", rule.tool_display().unwrap_or_default()));
        Ok(vec![it])
    }

    // ── scheduled cleaning ──

    /// Why a rule can't be scheduled (`None` = it can). Only Trash rules; caution never; review needs consent.
    pub fn schedule_problem(&self, rule_id: &str, review_ok: bool) -> Option<String> {
        let reg = self.registry();
        let Some(r) = reg.get(rule_id) else { return Some("Unknown rule".into()) };
        if r.method != Method::Trash || r.kind == Kind::Artifact || r.dynamic.is_some() {
            return Some("Only rules that move items to the Trash can be scheduled".into());
        }
        match r.risk {
            Risk::Caution => Some("Caution rules are never cleaned automatically".into()),
            Risk::Review if !review_ok => Some("Confirm that review rules may be cleaned automatically".into()),
            _ => None,
        }
    }

    /// Clean a rule without anyone watching: same scan, plan and run as a manual clean, but anything that
    /// would need confirmation (running apps, open files, caution) is skipped, and it is all logged.
    pub fn run_scheduled(&self, rule_id: &str, review_ok: bool, disabled: &HashSet<String>) -> Result<Report, String> {
        if let Some(why) = self.schedule_problem(rule_id, review_ok) {
            return Err(why);
        }
        if disabled.contains(rule_id) {
            return Err("The rule is switched off".into());
        }
        let reg = self.registry();
        let rule = reg.get(rule_id).ok_or("Unknown rule")?;
        let procs = self.sys.processes();
        let rr = self.scan_rule(rule, &procs);
        let ids: Vec<String> = rr.items.iter().filter(|i| i.blocked.is_none()).map(|i| i.id.clone()).collect();
        if ids.is_empty() {
            return Ok(Report { run: String::new(), dry_run: false, results: vec![], trashed_bytes: 0, freed_bytes: 0, ok: 0, failed: 0, skipped: 0, undo_log: self.undo.path().to_string_lossy().into_owned() });
        }
        let plan = self.plan_with(&[rr], &ids, &[], false, disabled);
        Ok(self.run(&plan, &RunOpts { trigger: "scheduled".into(), ..Default::default() }, &|_, _, _| {}))
    }

    // ── custom rules ──

    /// Validate a draft and show what it would match. Nothing is changed on disk.
    pub fn test_rule(&self, d: &crate::user_rules::Draft) -> RuleTest {
        use crate::user_rules::{check_shape, MAX_MATCHES};
        const MEASURE: usize = 200;
        let fail = |e: String, count| RuleTest { ok: false, error: Some(e), count, total: 0, measured: 0, items: vec![], blocked: vec![] };
        let locs = self.locations();
        if let Err(e) = check_shape(d, &locs, &self.safety) {
            return fail(e, 0);
        }
        let rule = d.to_rule();
        let paths = self.rule_paths(&rule);
        if paths.len() > MAX_MATCHES {
            return fail(format!("Matches {} items. Narrow it down to at most {MAX_MATCHES}.", paths.len()), paths.len());
        }
        let m = self.matcher(&rule);
        let blocked: Vec<TestItem> = paths
            .iter()
            .filter_map(|p| self.safety.check(p, &Allowed::Rule(&m)).err().map(|e| TestItem { path: p.to_string_lossy().into_owned(), size: 0, why: Some(e.to_string()) }))
            .collect();
        let mut sized: Vec<TestItem> = self.pool(|| {
            paths.par_iter().take(MEASURE).map(|p| TestItem { path: p.to_string_lossy().into_owned(), size: fsutil::measure(p).bytes, why: None }).collect()
        });
        sized.sort_by(|a, b| b.size.cmp(&a.size));
        let total = sized.iter().map(|i| i.size).sum();
        let measured = sized.len();
        sized.truncate(50);
        let error = (!blocked.is_empty()).then(|| format!("{} match(es) are protected and can never be cleaned, for example {}. Narrow the paths or add excludes.", blocked.len(), blocked[0].path));
        RuleTest { ok: blocked.is_empty(), error, count: paths.len(), total, measured, items: sized, blocked: blocked.into_iter().take(20).collect() }
    }

    // ── status (no measuring) ──

    pub fn status_all(&self) -> Vec<RuleStatus> {
        let _ = self.locations(); // resolve once before fanning out
        let reg = self.registry();
        self.pool(|| reg.rules.par_iter().map(|r| self.rule_status(r)).collect())
    }

    pub fn rule_status(&self, rule: &Rule) -> RuleStatus {
        let locations = self.rule_locations(rule);
        let st = |state, note: Option<String>, matches| RuleStatus { id: rule.id.clone(), state, note, locations: locations.clone(), matches };
        if rule.kind == Kind::Artifact {
            return st("on_demand", Some("Looks inside the project folders you choose".into()), 0);
        }
        if let Some(why) = self.not_installed(rule) {
            return st("not_installed", Some(why), 0);
        }
        if let Some(l) = locations.iter().find(|l| l.path.is_none()) {
            return if l.skipped.is_empty() {
                st("nothing_found", Some(format!("No {} folder on this Mac", l.id.replace('_', " "))), 0)
            } else {
                st("location_ignored", Some(l.skipped.join("; ")), 0)
            };
        }
        if rule.dynamic.as_deref() == Some("trash") {
            if let Err(e) = fs::read_dir(&self.trash_dir) {
                if e.raw_os_error() == Some(1) || e.kind() == std::io::ErrorKind::PermissionDenied {
                    return st("needs_fda", Some("Full Disk Access is needed to see the Trash".into()), 0);
                }
            }
        }
        if rule.paths.is_empty() {
            return st("ready", None, 0); // tool-only rules (docker, unavailable simulators)
        }
        let n = self.rule_paths(rule).len().min(1000);
        if n == 0 && self.blocked_by_privacy(rule) {
            st("needs_fda", Some("macOS blocks this folder: Full Disk Access is needed".into()), 0)
        } else if n == 0 {
            st("nothing_found", Some("Nothing to clean right now".into()), 0)
        } else {
            st("ready", None, n)
        }
    }

    /// Why the rule doesn't apply on this Mac: its app or tool is missing.
    fn not_installed(&self, rule: &Rule) -> Option<String> {
        if let Some(app) = &rule.app {
            if self.sys.app_installed(app) == Some(false) {
                return Some(format!("{} is not installed", rule.name.trim_end_matches(" cache")));
            }
        }
        if let Some(d) = &rule.detect {
            if !self.sys.detect(d) {
                return Some("Not installed on this Mac".into());
            }
        }
        None
    }

    /// A pattern's fixed part (the folder before the first wildcard) exists but macOS refuses to list it.
    fn blocked_by_privacy(&self, rule: &Rule) -> bool {
        let (inc, _) = self.expanded(rule);
        inc.iter().any(|p| {
            let fixed: PathBuf = Path::new(p).components().take_while(|c| !c.as_os_str().to_string_lossy().contains(['*', '?', '['])).collect();
            let dir = if fixed.is_dir() { fixed } else { fixed.parent().map(Path::to_path_buf).unwrap_or_default() };
            matches!(fs::read_dir(&dir), Err(e) if e.raw_os_error() == Some(1))
        })
    }

    // ── project build artifacts ──

    /// Find artifact folders below `roots`. Results are remembered so a plan can refer to them by id.
    pub fn find_artifacts(&self, roots: &[PathBuf], disabled: &HashSet<String>, cancel: &AtomicBool, dirs_seen: &std::sync::atomic::AtomicU64) -> (ArtifactScan, Vec<RuleResult>) {
        let roots: Vec<PathBuf> = roots.iter().filter_map(|r| r.canonicalize().ok()).filter(|r| r.is_dir()).collect();
        *self.art_roots.lock().unwrap() = roots.clone();
        let reg = self.registry();
        let rules: Vec<&Rule> = reg.rules.iter().filter(|r| r.kind == Kind::Artifact && !disabled.contains(&r.id)).collect();
        let procs = self.sys.processes();
        let found: Vec<(usize, PathBuf)> = self.pool(|| {
            roots.par_iter().flat_map(|r| walk_artifacts(r, 0, &rules, cancel, dirs_seen)).collect()
        });
        let guards: Vec<ArtifactMatcher> = rules.iter().map(|r| ArtifactMatcher::new(r, &roots)).collect();
        let measured: Vec<(usize, PathBuf, Measure)> = self.pool(|| found.into_par_iter().map(|(i, p)| { let m = fsutil::measure(&p); (i, p, m) }).collect());

        let mut by_project: std::collections::BTreeMap<PathBuf, Vec<ArtItem>> = Default::default();
        let mut per_rule: Vec<Vec<Item>> = rules.iter().map(|_| Vec::new()).collect();
        for (i, p, m) in measured {
            if m.files == 0 && m.bytes == 0 {
                continue;
            }
            let rule = rules[i];
            let blocked = self.safety.check(&p, &Allowed::Artifact(&guards[i])).err().map(|e| e.to_string());
            let id = format!("{}|{}", rule.id, p.to_string_lossy());
            let dir = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let project = p.parent().map(Path::to_path_buf).unwrap_or_default();
            by_project.entry(project).or_default().push(ArtItem { id: id.clone(), rule_id: rule.id.clone(), rule_name: rule.name.clone(), dir: dir.clone(), path: p.to_string_lossy().into_owned(), size: m.bytes, files: m.files, blocked: blocked.clone() });
            per_rule[i].push(Item { id, rule_id: rule.id.clone(), label: dir, path: Some(p.to_string_lossy().into_owned()), size: m.bytes, files: m.files, mtime: m.mtime, blocked, ..Default::default() });
        }
        let mut projects: Vec<Project> = by_project
            .into_iter()
            .map(|(path, mut items)| {
                items.sort_by(|a, b| b.size.cmp(&a.size));
                let total = items.iter().filter(|i| i.blocked.is_none()).map(|i| i.size).sum();
                let names: Vec<&str> = items.iter().map(|i| i.dir.as_str()).collect();
                let mut running = Vec::new();
                for it in &items {
                    if let Some(r) = reg.get(&it.rule_id) {
                        for b in running_matches(&procs, &r.busy, self.me) {
                            if !running.contains(&b) {
                                running.push(b);
                            }
                        }
                    }
                }
                Project { name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), last_active: project_activity(&path, &names), path: path.to_string_lossy().into_owned(), items, total, running }
            })
            .collect();
        projects.sort_by(|a, b| b.total.cmp(&a.total));
        let total = projects.iter().map(|p| p.total).sum();
        let results: Vec<RuleResult> = rules
            .iter()
            .zip(per_rule)
            .map(|(r, mut items)| {
                items.sort_by(|a, b| b.size.cmp(&a.size));
                RuleResult { rule: (*r).into(), locations: Vec::new(), available: true, note: None, total: items.iter().filter(|i| i.blocked.is_none()).map(|i| i.size).sum(), items, running: Vec::new() }
            })
            .collect();
        let scan = ArtifactScan { roots: roots.iter().map(|r| r.to_string_lossy().into_owned()).collect(), projects, total, dirs: dirs_seen.load(Relaxed) };
        (scan, results)
    }

    // ── planning (the preview) ──

    fn build_argv(rule: &Rule, extra: &Option<String>) -> Result<Vec<String>, String> {
        let tool = rule.tool.as_ref().ok_or("this rule has no tool command")?;
        tool.iter()
            .map(|a| match a.as_str() {
                "<udid>" => extra.as_deref().filter(|s| valid_udid(s)).map(str::to_string).ok_or_else(|| "invalid device id".to_string()),
                "<id>" => extra.as_deref().filter(|s| valid_id(s)).map(str::to_string).ok_or_else(|| "invalid runtime id".to_string()),
                s => Ok(s.to_string()),
            })
            .collect()
    }

    fn open_by(&self, open: &[(u32, String, PathBuf)], real: &Path) -> Vec<String> {
        let mut seen = BTreeSet::new();
        for (pid, cmd, p) in open {
            if *pid != self.me && p.starts_with(real) {
                seen.insert(format!("{cmd} ({pid})"));
            }
        }
        seen.into_iter().take(3).collect()
    }

    /// Build the verified preview. `ids` come from our own scan results; `picks` are paths the user chose.
    pub fn plan(&self, results: &[RuleResult], ids: &[String], picks: &[String], dry_run: bool) -> Plan {
        self.plan_with(results, ids, picks, dry_run, &HashSet::new())
    }

    /// Like `plan`, but items of rules the user switched off are refused.
    pub fn plan_with(&self, results: &[RuleResult], ids: &[String], picks: &[String], dry_run: bool, disabled: &HashSet<String>) -> Plan {
        let procs = self.sys.processes();
        let open = self.sys.open_files();
        let reg = self.registry();
        let mut items: Vec<PlanItem> = Vec::new();
        let mut seen = HashSet::new();

        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            let found = results.iter().find_map(|rr| rr.items.iter().find(|i| &i.id == id).map(|i| (rr, i)));
            let Some((rr, item)) = found else { continue };
            let Some(rule) = reg.get(&rr.rule.id) else { continue };
            let mut pi = self.plan_rule_item(rule, item, &procs, &open);
            if disabled.contains(&rule.id) {
                pi.blocked = Some("This rule is switched off in Settings".into());
            }
            items.push(pi);
        }
        for p in picks {
            if seen.insert(format!("pick|{p}")) {
                items.push(self.plan_pick(p, &open));
            }
        }
        let total = items.iter().filter(|i| i.blocked.is_none()).map(|i| i.size).sum();
        let erase = items.iter().any(|i| i.blocked.is_none() && i.method == Method::Erase);
        Plan { token: random_token(), dry_run, items, total, erase }
    }

    fn plan_rule_item(&self, rule: &Rule, item: &Item, procs: &[ProcInfo], open: &[(u32, String, PathBuf)]) -> PlanItem {
        let mut pi = PlanItem {
            id: item.id.clone(),
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            label: item.label.clone(),
            path: item.path.clone(),
            size: item.size,
            method: rule.method,
            risk: rule.risk,
            command: rule.tool_display(),
            recoverable: rule.method == Method::Trash,
            blocked: item.blocked.clone(),
            confirm: Vec::new(),
            admin_cmd: None,
            real: None,
            argv: None,
            source: Some(Source::Rule(rule.id.clone())),
        };
        let is_trash_rule = rule.dynamic.as_deref() == Some("trash");

        // 1. path safety, re-checked now (the item may be old)
        if let Some(p) = item.path.as_deref().map(Path::new).filter(|_| !is_trash_rule) {
            let g = self.guard(rule);
            match self.safety.check(p, &g.allowed()) {
                Ok(real) => pi.real = Some(real),
                Err(e) => pi.blocked = pi.blocked.or(Some(e.to_string())),
            }
        }
        // 2. tool command
        if rule.method == Method::Tool {
            match Self::build_argv(rule, &item.extra) {
                Ok(a) => {
                    pi.command = Some(a.join(" "));
                    if rule.needs_admin {
                        pi.admin_cmd = Some(format!("sudo {}", a.join(" ")));
                        pi.blocked = pi.blocked.or(Some("Needs administrator rights: run the command shown in Terminal yourself".into()));
                    }
                    pi.argv = Some(a);
                }
                Err(e) => pi.blocked = pi.blocked.or(Some(e)),
            }
        }
        // 3. running tools / apps
        let busy = running_matches(procs, &rule.busy, self.me);
        if !busy.is_empty() {
            if rule.category == "Browsers" {
                pi.blocked = pi.blocked.or(Some(format!("{} is running: quit it first", busy.join(", "))));
            } else {
                pi.confirm.push(format!("{} is running and may be using this", busy.join(", ")));
            }
        }
        // 4. open files
        if let Some(real) = &pi.real {
            let by = self.open_by(open, real);
            if !by.is_empty() {
                pi.confirm.push(format!("Files are open in {}", by.join(", ")));
            }
        }
        // 5. simulators: never a booted one (re-checked live)
        if rule.dynamic.as_deref() == Some("simctl_devices") {
            if let Some(udid) = &item.extra {
                match self.simctl_json(sv(["xcrun", "simctl", "list", "devices", "-j"])) {
                    Ok(v) => {
                        let state = v["devices"].as_object().into_iter().flatten().flat_map(|(_, a)| a.as_array().into_iter().flatten()).find(|d| d["udid"].as_str() == Some(udid)).and_then(|d| d["state"].as_str().map(str::to_string));
                        match state.as_deref() {
                            Some("Shutdown") => {}
                            Some(s) => pi.blocked = pi.blocked.or(Some(format!("{s}: shut it down in Simulator first"))),
                            None => pi.blocked = pi.blocked.or(Some("That simulator no longer exists".into())),
                        }
                    }
                    Err(e) => pi.blocked = pi.blocked.or(Some(format!("Could not check the simulator state: {e}"))),
                }
            }
        }
        // 6. risk and opt-in acknowledgements
        if rule.risk == Risk::Caution {
            pi.confirm.push("Caution: this cannot be easily rebuilt (see what happens if you delete it)".into());
        }
        if rule.opt_in {
            pi.confirm.push("This runs a tool you must opt in to explicitly".into());
        }
        if pi.method == Method::Tool && rule.dynamic.is_some() {
            pi.recoverable = false;
        }
        pi
    }

    fn plan_pick(&self, path: &str, open: &[(u32, String, PathBuf)]) -> PlanItem {
        let p = Path::new(path);
        let label = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        let mut pi = PlanItem {
            id: format!("pick|{path}"),
            rule_id: "pick".into(),
            rule_name: "Picked in Storage".into(),
            label,
            path: Some(path.to_string()),
            size: 0,
            method: Method::Trash,
            risk: Risk::Review,
            command: None,
            recoverable: true,
            blocked: None,
            confirm: Vec::new(),
            admin_cmd: None,
            real: None,
            argv: None,
            source: Some(Source::Pick),
        };
        match self.safety.check(p, &Allowed::UserPick) {
            Ok(real) => {
                pi.size = self.pool(|| fsutil::measure(&real)).bytes;
                let by = self.open_by(open, &real);
                if !by.is_empty() {
                    pi.confirm.push(format!("Files are open in {}", by.join(", ")));
                }
                pi.real = Some(real);
            }
            Err(e) => pi.blocked = Some(e.to_string()),
        }
        pi
    }

    // ── running ──

    fn discover_trash(&self, before: &BTreeSet<OsString>, name: &OsStr) -> Option<PathBuf> {
        let after = fsutil::list_names(&self.trash_dir);
        let new: Vec<&OsString> = after.difference(before).collect();
        let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        new.iter().find(|n| n.as_os_str() == name).or_else(|| new.iter().find(|n| n.to_string_lossy().starts_with(&stem))).or(new.first().filter(|_| new.len() == 1)).map(|n| self.trash_dir.join(n))
    }

    fn rule_bytes(&self, rule: &Rule) -> u64 {
        self.pool(|| self.rule_paths(rule).par_iter().map(|p| fsutil::measure(p).bytes).sum())
    }

    /// Execute a verified plan. Everything the plan flagged is re-checked first.
    pub fn run(&self, plan: &Plan, opts: &RunOpts, progress: &dyn Fn(usize, usize, &str)) -> Report {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let run = format!("{:x}", ts * 1000 + (self.me as u64 % 1000));
        let reg = self.registry();
        let procs = self.sys.processes();
        let mut report = Report { run: run.clone(), dry_run: plan.dry_run, results: Vec::new(), trashed_bytes: 0, freed_bytes: 0, ok: 0, failed: 0, skipped: 0, undo_log: self.undo.path().to_string_lossy().into_owned() };
        let mut log: Vec<Entry> = Vec::new();
        let total = plan.items.len();

        for (n, it) in plan.items.iter().enumerate() {
            progress(n, total, &it.label);
            let mut res = ItemResult { id: it.id.clone(), label: it.label.clone(), path: it.path.clone(), status: "ok".into(), method: it.method, bytes: 0, trash_path: None, error: None };
            let skip = |res: &mut ItemResult, why: String| {
                res.status = "skipped".into();
                res.error = Some(why);
            };

            let rule = match &it.source {
                Some(Source::Rule(id)) => reg.get(id),
                _ => None,
            };
            if let Some(why) = &it.blocked {
                skip(&mut res, why.clone());
            } else if !it.confirm.is_empty() && !opts.overrides.contains(&it.id) {
                skip(&mut res, "Needs your confirmation in the preview".into());
            } else if it.method == Method::Erase && !opts.confirm_erase {
                skip(&mut res, "Permanent deletion was not confirmed".into());
            } else if rule.is_some_and(|r| r.category == "Browsers" && !running_matches(&procs, &r.busy, self.me).is_empty()) {
                skip(&mut res, "The browser was started after the preview: quit it and try again".into());
            } else if plan.dry_run {
                res.status = "dry-run".into();
                res.bytes = it.size;
            } else {
                match self.execute(it, rule) {
                    Ok(ex) => {
                        res.bytes = ex.bytes;
                        res.trash_path = ex.trash_path;
                        match it.method {
                            Method::Trash => report.trashed_bytes += ex.bytes,
                            _ => report.freed_bytes += ex.bytes,
                        }
                    }
                    Err(e) => {
                        res.status = "failed".into();
                        res.error = Some(e);
                    }
                }
            }

            match res.status.as_str() {
                "ok" | "dry-run" => report.ok += 1,
                "failed" => report.failed += 1,
                _ => report.skipped += 1,
            }
            if res.status != "skipped" {
                let mut e = Entry::new(&run, ts, n, &it.rule_id, &it.label, if plan.dry_run { "dry-run" } else { method_name(it.method) });
                e.path = it.path.clone();
                e.trash_path = res.trash_path.clone();
                e.bytes = res.bytes;
                e.command = it.argv.clone();
                e.status = if res.status == "failed" { "failed".into() } else { "ok".into() };
                e.error = res.error.clone();
                if !opts.trigger.is_empty() {
                    e.trigger = opts.trigger.clone();
                }
                log.push(e);
            }
            report.results.push(res);
        }
        progress(total, total, "");
        let _ = self.undo.append(&log);
        report
    }

    fn execute(&self, it: &PlanItem, rule: Option<&Rule>) -> Result<Executed, String> {
        match it.method {
            Method::Trash => {
                let real = it.real.as_ref().ok_or("no verified path")?;
                // Last look: the path must still pass the gate, exactly as at preview time.
                let guard = rule.map(|r| self.guard(r));
                let allowed = match (&it.source, &guard) {
                    (Some(Source::Rule(_)), Some(g)) => g.allowed(),
                    _ => Allowed::UserPick,
                };
                let real_now = self.safety.check(real, &allowed).map_err(|d| d.to_string())?;
                let before = fsutil::list_names(&self.trash_dir);
                let dest = self.sys.trash(&real_now).map_err(|e| e.to_string())?;
                let name = real_now.file_name().unwrap_or_default().to_owned();
                // Prefer the path the OS reported; fall back to diffing the Trash folder when it can be listed.
                let trash_path = dest.or_else(|| self.discover_trash(&before, &name));
                Ok(Executed { bytes: it.size, trash_path: trash_path.map(|p| p.to_string_lossy().into_owned()) })
            }
            Method::Tool => {
                let argv = it.argv.as_ref().ok_or("no verified command")?;
                let rule = rule.ok_or("unknown rule")?;
                let free0 = self.sys.free_bytes(self.home());
                let before = (!rule.paths.is_empty()).then(|| self.rule_bytes(rule));
                self.sys.run_tool(argv)?;
                let freed = match before {
                    Some(b) => b.saturating_sub(self.rule_bytes(rule)),
                    None => self.sys.free_bytes(self.home()).saturating_sub(free0),
                };
                Ok(Executed { bytes: freed, trash_path: None })
            }
            Method::Erase => {
                let rule = rule.ok_or("unknown rule")?;
                if rule.dynamic.as_deref() != Some("trash") {
                    return Err("Only the Trash can be erased in place".into());
                }
                let m = self.matcher(rule);
                let mut bytes = 0;
                let mut errors = 0;
                for e in fs::read_dir(&self.trash_dir).map_err(|e| e.to_string())?.flatten() {
                    let p = e.path();
                    if self.safety.check(&p, &Allowed::Rule(&m)).is_err() {
                        errors += 1;
                        continue;
                    }
                    let size = self.pool(|| fsutil::measure(&p)).bytes;
                    let ft = e.file_type().map_err(|e| e.to_string())?;
                    let r = if ft.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                    match r {
                        Ok(()) => bytes += size,
                        Err(_) => errors += 1,
                    }
                }
                if errors > 0 {
                    return Err(format!("{errors} item(s) could not be removed (macOS may need Full Disk Access)"));
                }
                Ok(Executed { bytes, trash_path: None })
            }
        }
    }

    /// Put an item back from the Trash where it came from.
    pub fn restore(&self, entry_id: &str) -> Result<String, String> {
        let e = self.undo.get(entry_id).ok_or("That log entry no longer exists")?;
        if e.action != "trash" || e.status != "ok" {
            return Err("Only items moved to the Trash can be restored".into());
        }
        if e.restored {
            return Err("Already restored".into());
        }
        let (orig, tp) = (PathBuf::from(e.path.ok_or("No original path recorded")?), PathBuf::from(e.trash_path.ok_or("Deskveet could not tell where macOS put this item: look for it in the Trash in Finder")?));
        if !orig.is_absolute() || tp.parent() != Some(self.trash_dir.as_path()) {
            return Err("The log entry does not point into the Trash".into());
        }
        if fs::symlink_metadata(&tp).is_err() {
            return Err("It is no longer in the Trash (it may have been emptied)".into());
        }
        if fs::symlink_metadata(&orig).is_ok() {
            return Err("Something already exists at the original location".into());
        }
        let parent = orig.parent().ok_or("bad path")?;
        if !parent.is_dir() {
            return Err("The original folder no longer exists".into());
        }
        fs::rename(&tp, &orig).map_err(|e| e.to_string())?;
        let _ = self.undo.mark_restored(entry_id);
        Ok(orig.to_string_lossy().into_owned())
    }
}

struct Executed {
    bytes: u64,
    trash_path: Option<String>,
}

fn method_name(m: Method) -> &'static str {
    match m {
        Method::Trash => "trash",
        Method::Tool => "tool",
        Method::Erase => "erase",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::ProcInfo;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Records everything and never touches the real system. `trash` moves into a temp "Trash".
    struct FakeSys {
        trash_dir: PathBuf,
        procs: Mutex<Vec<ProcInfo>>,
        open: Mutex<Vec<(u32, String, PathBuf)>>,
        missing_tools: Mutex<HashSet<String>>,
        captures: Mutex<HashMap<String, String>>,
        tool_log: Mutex<Vec<Vec<String>>>,
        fail_tools: Mutex<bool>,
        /// false = behave like a system that cannot report the Trash path (forces the fallback).
        report_dest: Mutex<bool>,
        env: Mutex<HashMap<String, String>>,
        missing_apps: Mutex<HashSet<String>>,
    }

    impl Sys for FakeSys {
        fn detect(&self, cmd: &str) -> bool {
            !self.missing_tools.lock().unwrap().iter().any(|m| cmd.contains(m.as_str()))
        }
        fn capture(&self, argv: &[String]) -> Result<String, String> {
            self.captures.lock().unwrap().get(&argv.join(" ")).cloned().ok_or_else(|| format!("no fixture for {}", argv.join(" ")))
        }
        fn run_tool(&self, argv: &[String]) -> Result<String, String> {
            self.tool_log.lock().unwrap().push(argv.to_vec());
            if *self.fail_tools.lock().unwrap() { Err("boom".into()) } else { Ok("ok".into()) }
        }
        fn processes(&self) -> Vec<ProcInfo> {
            self.procs.lock().unwrap().clone()
        }
        fn open_files(&self) -> Vec<(u32, String, PathBuf)> {
            self.open.lock().unwrap().clone()
        }
        fn trash(&self, path: &Path) -> io::Result<Option<PathBuf>> {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut dest = self.trash_dir.join(&name);
            let mut n = 2;
            while fs::symlink_metadata(&dest).is_ok() {
                dest = self.trash_dir.join(format!("{name} {n}"));
                n += 1;
            }
            fs::rename(path, &dest)?;
            Ok(if *self.report_dest.lock().unwrap() { Some(dest) } else { None })
        }
        fn free_bytes(&self, _: &Path) -> u64 {
            1_000
        }
        fn env(&self) -> HashMap<String, String> {
            self.env.lock().unwrap().clone()
        }
        fn app_installed(&self, id: &str) -> Option<bool> {
            Some(!self.missing_apps.lock().unwrap().contains(id))
        }
    }

    struct Fx {
        _t: TempDir,
        home: PathBuf,
        sys: Arc<FakeSys>,
        eng: Engine,
    }

    fn put(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![9u8; bytes]).unwrap();
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap().join("home");
        let trash = home.join(".Trash");
        fs::create_dir_all(&trash).unwrap();
        for d in ["Documents", "Desktop"] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        let sys = Arc::new(FakeSys {
            trash_dir: trash.clone(),
            procs: Mutex::new(vec![]),
            open: Mutex::new(vec![]),
            missing_tools: Mutex::new(HashSet::new()),
            captures: Mutex::new(HashMap::new()),
            tool_log: Mutex::new(vec![]),
            fail_tools: Mutex::new(false),
            report_dest: Mutex::new(true),
            env: Mutex::new(HashMap::new()),
            missing_apps: Mutex::new(HashSet::new()),
        });
        let eng = Engine::new(Registry::macos().unwrap(), home.clone(), sys.clone(), trash, t.path().join("undo.json"));
        Fx { _t: t, home, sys, eng }
    }

    fn scan(f: &Fx, rule: &str) -> RuleResult {
        let r = f.eng.registry().get(rule).unwrap().clone();
        f.eng.scan_rule(&r, &f.sys.processes())
    }

    fn proc(name: &str) -> ProcInfo {
        ProcInfo { pid: 4242, name: name.into(), cmd: name.into() }
    }

    fn ids(rr: &RuleResult) -> Vec<String> {
        rr.items.iter().map(|i| i.id.clone()).collect()
    }

    fn run(f: &Fx, plan: &Plan, opts: &RunOpts) -> Report {
        f.eng.run(plan, opts, &|_, _, _| {})
    }

    #[test]
    fn scan_lists_items_with_sizes_and_hides_empty_ones() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("AppA-abc/Build/big.o"), 300_000);
        put(&dd.join("AppB-def/Build/small.o"), 50_000);
        fs::create_dir_all(dd.join("Empty-xyz")).unwrap();
        let rr = scan(&f, "xcode.derived_data");
        assert!(rr.available);
        assert_eq!(rr.items.len(), 2, "empty folder hidden");
        assert!(rr.items[0].label.starts_with("AppA") && rr.items[0].size >= 300_000, "sorted by size");
        assert_eq!(rr.total, rr.items.iter().map(|i| i.size).sum::<u64>());
        // a rule whose folders do not exist yields nothing, not an error
        assert!(scan(&f, "xcode.archives").items.is_empty());
    }

    #[test]
    fn trash_run_moves_items_logs_them_and_reports_bytes() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("AppA-abc/x"), 200_000);
        put(&dd.join("AppB-def/y"), 100_000);
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert_eq!(plan.items.len(), 2);
        assert!(plan.items.iter().all(|i| i.blocked.is_none() && i.confirm.is_empty() && i.recoverable));
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!((rep.ok, rep.failed, rep.skipped), (2, 0, 0));
        assert!(rep.trashed_bytes >= 300_000 && rep.freed_bytes == 0);
        assert!(!dd.join("AppA-abc").exists() && !dd.join("AppB-def").exists());
        assert!(f.home.join(".Trash/AppA-abc/x").exists());
        let log = f.eng.undo.list();
        assert_eq!(log.len(), 2);
        assert!(log.iter().all(|e| e.action == "trash" && e.status == "ok" && e.trash_path.is_some() && e.path.is_some()));
        // and the log can put it back
        let a = log.iter().find(|e| e.label.starts_with("AppA")).unwrap();
        f.eng.restore(&a.id).unwrap();
        assert!(dd.join("AppA-abc/x").exists());
        assert!(f.eng.restore(&a.id).is_err(), "cannot restore twice");
    }

    #[test]
    fn trash_path_falls_back_to_diffing_the_trash_when_the_os_does_not_report_it() {
        let f = fx();
        *f.sys.report_dest.lock().unwrap() = false;
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        run(&f, &plan, &RunOpts::default());
        let e = f.eng.undo.list().remove(0);
        assert_eq!(e.trash_path.as_deref(), Some(f.home.join(".Trash/A-1").to_str().unwrap()));
    }

    #[test]
    fn restore_refuses_when_original_exists_or_trash_item_is_gone() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        run(&f, &plan, &RunOpts::default());
        let e = f.eng.undo.list().remove(0);
        fs::create_dir_all(dd.join("A-1")).unwrap();
        assert!(f.eng.restore(&e.id).unwrap_err().contains("already exists"));
        fs::remove_dir(dd.join("A-1")).unwrap();
        fs::remove_dir_all(e.trash_path.as_ref().unwrap()).unwrap();
        assert!(f.eng.restore(&e.id).unwrap_err().contains("no longer in the Trash"));
    }

    #[test]
    fn dry_run_changes_nothing_but_is_logged() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], true);
        let rep = run(&f, &plan, &RunOpts::default());
        assert!(rep.dry_run && rep.ok == 1 && rep.trashed_bytes == 0);
        assert!(dd.join("A-1/x").exists(), "nothing moved");
        assert!(fs::read_dir(f.home.join(".Trash")).unwrap().next().is_none());
        let log = f.eng.undo.list();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].action, "dry-run");
        assert!(f.sys.tool_log.lock().unwrap().is_empty());
    }

    #[test]
    fn running_xcode_needs_confirmation_and_open_files_are_flagged() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        f.sys.procs.lock().unwrap().push(proc("Xcode"));
        f.sys.open.lock().unwrap().push((777, "clang".into(), dd.join("A-1/Build/obj.o")));
        let rr = scan(&f, "xcode.derived_data");
        assert_eq!(rr.running, vec!["Xcode".to_string()]);
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        let it = &plan.items[0];
        assert!(it.blocked.is_none());
        assert!(it.confirm.iter().any(|c| c.contains("Xcode is running")));
        assert!(it.confirm.iter().any(|c| c.contains("clang (777)")));
        // without acknowledgement it is skipped, nothing moves
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!((rep.ok, rep.skipped), (0, 1));
        assert!(dd.join("A-1/x").exists());
        // an override for some other id does not help
        let mut o = RunOpts::default();
        o.overrides.insert("something|else".into());
        assert_eq!(run(&f, &plan, &o).skipped, 1);
        // acknowledged: it goes ahead
        let mut o = RunOpts::default();
        o.overrides.insert(it.id.clone());
        assert_eq!(run(&f, &plan, &o).ok, 1);
        assert!(!dd.join("A-1").exists());
    }

    #[test]
    fn browser_caches_are_hard_blocked_while_the_browser_runs() {
        let f = fx();
        put(&f.home.join("Library/Application Support/Google/Chrome/Default/Cache/data_0"), 200_000);
        put(&f.home.join("Library/Application Support/Google/Chrome/Default/Cookies"), 5_000);
        f.sys.procs.lock().unwrap().push(proc("Google Chrome"));
        let rr = scan(&f, "browser.chrome");
        assert_eq!(rr.items.len(), 1, "only the Cache folder, never Cookies");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert!(plan.items[0].blocked.as_deref().unwrap().contains("quit it first"));
        let mut o = RunOpts::default();
        o.overrides.insert(plan.items[0].id.clone());
        let rep = run(&f, &plan, &o);
        assert_eq!(rep.skipped, 1, "no override can unblock a browser");
        assert!(f.home.join("Library/Application Support/Google/Chrome/Default/Cache/data_0").exists());
        // browser closed -> allowed
        f.sys.procs.lock().unwrap().clear();
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert_eq!(run(&f, &plan, &RunOpts::default()).ok, 1);
        assert!(f.home.join("Library/Application Support/Google/Chrome/Default/Cookies").exists());
    }

    #[test]
    fn explicit_picks_go_to_trash_and_blocked_picks_are_refused() {
        let f = fx();
        let file = f.home.join("Documents/big-video.mov");
        put(&file, 400_000);
        let picks = vec![
            file.to_string_lossy().into_owned(),
            f.home.join("Documents").to_string_lossy().into_owned(),
            f.home.join("Desktop").to_string_lossy().into_owned(),
            "/System/Library".to_string(),
            "/".to_string(),
        ];
        let plan = f.eng.plan(&[], &[], &picks, false);
        assert_eq!(plan.items.len(), 5);
        assert!(plan.items[0].blocked.is_none() && plan.items[0].method == Method::Trash && plan.items[0].size >= 400_000);
        assert!(plan.items[1..].iter().all(|i| i.blocked.is_some()), "{:?}", plan.items.iter().map(|i| &i.blocked).collect::<Vec<_>>());
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!((rep.ok, rep.skipped), (1, 4));
        assert!(!file.exists() && f.home.join(".Trash/big-video.mov").exists());
        assert!(f.home.join("Documents").exists() && f.home.join("Desktop").exists());
    }

    #[test]
    fn tool_rules_run_the_registry_command_only_when_detected() {
        let f = fx();
        put(&f.home.join("Library/Caches/Homebrew/downloads/a.tar.gz"), 300_000);
        let rr = scan(&f, "brew.cache");
        assert_eq!(rr.items.len(), 1);
        assert!(rr.items[0].size >= 300_000);
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert_eq!(plan.items[0].command.as_deref(), Some("brew cleanup -s"));
        assert!(!plan.items[0].recoverable);
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!(rep.ok, 1);
        assert_eq!(f.sys.tool_log.lock().unwrap().as_slice(), &[sv(["brew", "cleanup", "-s"])]);
        assert!(f.eng.undo.list()[0].command.is_some());
        // a failing tool is reported, not hidden
        *f.sys.fail_tools.lock().unwrap() = true;
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!((rep.failed, rep.results[0].error.as_deref()), (1, Some("boom")));
        // not installed -> rule unavailable, no items
        f.sys.missing_tools.lock().unwrap().insert("brew".into());
        let rr = scan(&f, "brew.cache");
        assert!(!rr.available && rr.items.is_empty());
    }

    #[test]
    fn docker_prune_needs_an_explicit_acknowledgement() {
        let f = fx();
        let rr = scan(&f, "docker.prune");
        assert_eq!(rr.items.len(), 1);
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert!(plan.items[0].confirm.iter().any(|c| c.contains("opt in")));
        assert_eq!(run(&f, &plan, &RunOpts::default()).skipped, 1);
        assert!(f.sys.tool_log.lock().unwrap().is_empty());
        let mut o = RunOpts::default();
        o.overrides.insert(plan.items[0].id.clone());
        assert_eq!(run(&f, &plan, &o).ok, 1);
        assert_eq!(f.sys.tool_log.lock().unwrap()[0], sv(["docker", "system", "prune", "-f"]));
    }

    fn sim_fixture(f: &Fx) -> (String, String) {
        let (u1, u2) = ("11111111-1111-1111-1111-111111111111".to_string(), "22222222-2222-2222-2222-222222222222".to_string());
        put(&f.home.join(format!("Library/Developer/CoreSimulator/Devices/{u1}/data/a")), 200_000);
        put(&f.home.join(format!("Library/Developer/CoreSimulator/Devices/{u2}/data/b")), 100_000);
        let devices = format!(
            r#"{{"devices":{{"com.apple.CoreSimulator.SimRuntime.iOS-17-5":[
              {{"udid":"{u1}","name":"iPhone 15","state":"Shutdown","isAvailable":true,"lastBootedAt":"2026-01-02T03:04:05Z"}},
              {{"udid":"{u2}","name":"iPhone 15 Pro","state":"Booted","isAvailable":true}},
              {{"udid":"not-a-uuid","name":"Evil; rm -rf","state":"Shutdown","isAvailable":true}}],
             "com.apple.CoreSimulator.SimRuntime.tvOS-17-0":[]}}}}"#
        );
        let runtimes = r#"{"AAAAAAAA-0000-0000-0000-000000000001":{"identifier":"AAAAAAAA-0000-0000-0000-000000000001","runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-17-5","build":"21F79","sizeBytes":7000000000,"deletable":true,"state":"Ready"},
                          "BBBBBBBB-0000-0000-0000-000000000002":{"identifier":"BBBBBBBB-0000-0000-0000-000000000002","runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.tvOS-17-0","build":"21J1","sizeBytes":3000000000,"deletable":false,"state":"Ready"}}"#;
        let mut c = f.sys.captures.lock().unwrap();
        c.insert("xcrun simctl list devices -j".into(), devices);
        c.insert("xcrun simctl runtime list -j".into(), runtimes.into());
        (u1, u2)
    }

    #[test]
    fn simulator_devices_booted_is_never_touched_and_ids_are_validated() {
        let f = fx();
        let (u1, u2) = sim_fixture(&f);
        let rr = scan(&f, "simulators.devices");
        assert_eq!(rr.items.len(), 2, "the malformed udid is ignored entirely");
        let booted = rr.items.iter().find(|i| i.id.ends_with(&u2)).unwrap();
        assert!(booted.blocked.as_deref().unwrap().contains("Booted"));
        let ok = rr.items.iter().find(|i| i.id.ends_with(&u1)).unwrap();
        assert!(ok.blocked.is_none() && ok.label.contains("iPhone 15 · iOS 17.5") && ok.info.iter().any(|s| s.contains("2026-01-02")));
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!((rep.ok, rep.skipped), (1, 1));
        let calls = f.sys.tool_log.lock().unwrap().clone();
        assert_eq!(calls, vec![sv(["xcrun", "simctl", "delete", &u1])]);
        // the device boots between scan and run: the plan re-checks live and refuses
        f.sys.captures.lock().unwrap().insert("xcrun simctl list devices -j".into(), format!(r#"{{"devices":{{"k":[{{"udid":"{u1}","state":"Booted","name":"x"}}]}}}}"#));
        let plan2 = f.eng.plan(&[rr.clone()], &[ok.id.clone()], &[], false);
        assert!(plan2.items[0].blocked.as_deref().unwrap().contains("Booted"));
    }

    #[test]
    fn simulator_runtimes_show_users_and_respect_deletable() {
        let f = fx();
        sim_fixture(&f);
        let rr = scan(&f, "simulators.runtimes");
        assert_eq!(rr.items.len(), 2);
        let ios = rr.items.iter().find(|i| i.label.starts_with("iOS 17.5")).unwrap();
        assert_eq!(ios.size, 7_000_000_000);
        assert!(ios.info.iter().any(|s| s.contains("3 simulator device(s) use it")), "{:?}", ios.info);
        let tv = rr.items.iter().find(|i| i.label.starts_with("tvOS")).unwrap();
        assert!(tv.blocked.is_some());
        let plan = f.eng.plan(&[rr.clone()], &[ios.id.clone()], &[], false);
        run(&f, &plan, &RunOpts::default());
        assert_eq!(f.sys.tool_log.lock().unwrap()[0], sv(["xcrun", "simctl", "runtime", "delete", "AAAAAAAA-0000-0000-0000-000000000001"]));
    }

    #[test]
    fn emptying_the_trash_needs_the_second_confirmation_and_only_touches_the_trash() {
        let f = fx();
        put(&f.home.join(".Trash/old.zip"), 200_000);
        put(&f.home.join(".Trash/folder/inner.txt"), 100_000);
        put(&f.home.join("Documents/keep.txt"), 1_000);
        let rr = scan(&f, "system.trash");
        assert_eq!(rr.items.len(), 1);
        let mut plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert!(plan.erase);
        plan.items[0].confirm.clear(); // pretend the caution tick-box was ticked in the UI
        let mut o = RunOpts::default();
        let rep = run(&f, &plan, &o);
        assert_eq!(rep.skipped, 1);
        assert!(f.home.join(".Trash/old.zip").exists(), "not erased without the second confirmation");
        o.confirm_erase = true;
        let rep = run(&f, &plan, &o);
        assert_eq!((rep.ok, rep.failed), (1, 0));
        assert!(rep.freed_bytes >= 300_000 && rep.trashed_bytes == 0);
        assert!(fs::read_dir(f.home.join(".Trash")).unwrap().next().is_none());
        assert!(f.home.join(".Trash").is_dir(), "the Trash folder itself stays");
        assert!(f.home.join("Documents/keep.txt").exists());
    }

    #[test]
    fn ids_unknown_to_the_scan_are_ignored_and_paths_cannot_be_smuggled_in_via_ids() {
        let f = fx();
        put(&f.home.join("Documents/precious.txt"), 1_000);
        let rr = scan(&f, "xcode.derived_data");
        let evil = format!("xcode.derived_data|{}", f.home.join("Documents/precious.txt").display());
        let plan = f.eng.plan(&[rr], &[evil, "nonexistent|x".into()], &[], false);
        assert!(plan.items.is_empty());
    }

    #[test]
    fn item_gone_or_replaced_between_preview_and_run_is_handled() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        fs::remove_dir_all(dd.join("A-1")).unwrap();
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!(rep.failed, 1, "vanished item is a clean failure, not a panic");
        // swap the whole DerivedData folder for a symlink to Documents: the real path is no longer allowed
        put(&dd.join("B-2/x"), 100_000);
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        fs::rename(&dd, f.home.join("dd-moved")).unwrap();
        std::os::unix::fs::symlink(f.home.join("Documents"), &dd).unwrap();
        put(&f.home.join("Documents/B-2/x"), 5);
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!(rep.ok, 0);
        assert!(f.home.join("Documents/B-2/x").exists(), "Documents untouched");
    }

    fn art_scan(f: &Fx) -> (ArtifactScan, Vec<RuleResult>) {
        let seen = std::sync::atomic::AtomicU64::new(0);
        f.eng.find_artifacts(&[f.home.join("dev")], &HashSet::new(), &AtomicBool::new(false), &seen)
    }

    fn make_projects(f: &Fx) -> PathBuf {
        let d = f.home.join("dev");
        put(&d.join("web/package.json"), 10);
        put(&d.join("web/node_modules/lib/index.js"), 300_000);
        put(&d.join("web/node_modules/lib/node_modules/inner/x.js"), 100_000); // nested: never listed separately
        put(&d.join("web/src/app.js"), 10);
        put(&d.join("engine/Cargo.toml"), 10);
        put(&d.join("engine/target/debug/bin"), 400_000);
        put(&d.join("mobile/pubspec.yaml"), 10);
        put(&d.join("mobile/build/app.apk"), 200_000);
        put(&d.join("mobile/.dart_tool/x"), 50_000);
        put(&d.join("notes/build/readme.txt"), 100_000); // `build` without a marker: not an artifact
        put(&d.join("web/.git/objects/pack"), 100_000);
        put(&d.join("web/.git/node_modules/trap/x"), 100_000); // inside .git: never visited
        d
    }

    #[test]
    fn artifact_finder_groups_by_project_and_needs_marker_files() {
        let f = fx();
        make_projects(&f);
        let (scan, results) = art_scan(&f);
        let names: Vec<&str> = scan.projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"web") && names.contains(&"engine") && names.contains(&"mobile"));
        assert!(!names.contains(&"notes"), "build/ without a marker is not an artifact");
        let web = scan.projects.iter().find(|p| p.name == "web").unwrap();
        assert_eq!(web.items.len(), 1, "nested node_modules and .git contents are not separate items");
        assert!(web.items[0].size >= 400_000, "nested folder is counted inside its parent");
        let mobile = scan.projects.iter().find(|p| p.name == "mobile").unwrap();
        let rules: HashSet<&str> = mobile.items.iter().map(|i| i.rule_id.as_str()).collect();
        assert_eq!(rules, HashSet::from(["artifact.flutter_build", "artifact.dart_tool"]));
        assert!(scan.projects.windows(2).all(|w| w[0].total >= w[1].total));
        assert!(results.iter().any(|r| r.rule.id == "artifact.rust_target" && r.items.len() == 1));
        assert!(scan.projects.iter().all(|p| p.last_active > 0));
    }

    #[test]
    fn artifact_items_go_to_trash_through_the_same_plan_and_undo_log() {
        let f = fx();
        let d = make_projects(&f);
        let (_scan, results) = art_scan(&f);
        let all: Vec<String> = results.iter().flat_map(|r| r.items.iter().map(|i| i.id.clone())).collect();
        let plan = f.eng.plan(&results, &all, &[], false);
        assert!(plan.items.iter().all(|i| i.blocked.is_none()), "{:?}", plan.items.iter().map(|i| &i.blocked).collect::<Vec<_>>());
        let rep = run(&f, &plan, &RunOpts::default());
        assert_eq!((rep.failed, rep.skipped), (0, 0));
        assert!(!d.join("web/node_modules").exists() && !d.join("engine/target").exists() && !d.join("mobile/build").exists());
        assert!(d.join("web/src/app.js").exists() && d.join("web/package.json").exists() && d.join("notes/build/readme.txt").exists());
        assert!(d.join("web/.git/objects/pack").exists());
        assert!(f.eng.undo.list().iter().all(|e| e.action == "trash" && e.trash_path.is_some()));
    }

    #[test]
    fn artifact_plan_is_refused_when_the_marker_disappears_or_it_is_outside_the_chosen_roots() {
        let f = fx();
        let d = make_projects(&f);
        let (_s, results) = art_scan(&f);
        let nm = results.iter().flat_map(|r| &r.items).find(|i| i.label == "node_modules").unwrap().id.clone();
        fs::remove_file(d.join("web/package.json")).unwrap();
        let plan = f.eng.plan(&results, &[nm.clone()], &[], false);
        assert!(plan.items[0].blocked.is_some(), "no package.json next to it any more");
        assert_eq!(run(&f, &plan, &RunOpts::default()).skipped, 1);
        assert!(d.join("web/node_modules").exists());
        // a later scan of another folder invalidates items from the old one
        put(&d.join("web/package.json"), 10);
        f.eng.find_artifacts(&[f.home.join("Desktop")], &HashSet::new(), &AtomicBool::new(false), &std::sync::atomic::AtomicU64::new(0));
        let plan = f.eng.plan(&results, &[nm], &[], false);
        assert!(plan.items[0].blocked.is_some(), "outside the scanned roots");
    }

    #[test]
    fn running_node_needs_confirmation_for_node_modules() {
        let f = fx();
        make_projects(&f);
        f.sys.procs.lock().unwrap().push(proc("node"));
        let (scan, results) = art_scan(&f);
        assert!(scan.projects.iter().any(|p| p.running.contains(&"node".to_string())));
        let nm = results.iter().flat_map(|r| &r.items).find(|i| i.label == "node_modules").unwrap().id.clone();
        let plan = f.eng.plan(&results, &[nm], &[], false);
        assert!(plan.items[0].confirm.iter().any(|c| c.contains("node is running")));
    }

    #[test]
    fn switched_off_rules_are_not_scanned_and_their_items_are_refused() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        put(&f.home.join("dev/web/package.json"), 10);
        put(&f.home.join("dev/web/node_modules/x.js"), 100_000);
        let off: HashSet<String> = ["xcode.derived_data".to_string(), "artifact.node_modules".to_string()].into();
        let all = f.eng.scan_all(&AtomicBool::new(false), &off, &|_| {});
        assert!(all.iter().all(|r| r.rule.id != "xcode.derived_data"));
        let (art, _) = f.eng.find_artifacts(&[f.home.join("dev")], &off, &AtomicBool::new(false), &std::sync::atomic::AtomicU64::new(0));
        assert!(art.projects.is_empty());
        // an item scanned while the rule was on is refused once it is switched off
        let rr = scan(&f, "xcode.derived_data");
        let plan = f.eng.plan_with(&[rr.clone()], &ids(&rr), &[], false, &off);
        assert!(plan.items[0].blocked.as_deref().unwrap().contains("switched off"));
        assert_eq!(run(&f, &plan, &RunOpts::default()).skipped, 1);
        assert!(dd.join("A-1/x").exists());
    }

    #[test]
    fn rules_follow_moved_tool_folders_from_env_and_tool_queries() {
        let f = fx();
        // Gradle moved with GRADLE_USER_HOME, the Android SDK via ANDROID_HOME, Go's cache via `go env`.
        let gradle = f.home.join("dev/gradle-home");
        put(&gradle.join("caches/modules-2/x.jar"), 200_000);
        let sdk = f.home.join("dev/android-sdk");
        put(&sdk.join("system-images/android-34/google_apis/img"), 300_000);
        let gocache = f.home.join("dev/go-cache");
        put(&gocache.join("ab/cd"), 100_000);
        put(&f.home.join(".gradle/caches/default-only/y"), 50_000); // the default must now be ignored
        f.sys.env.lock().unwrap().extend([
            ("GRADLE_USER_HOME".to_string(), gradle.to_string_lossy().into_owned()),
            ("ANDROID_HOME".to_string(), sdk.to_string_lossy().into_owned()),
        ]);
        f.sys.captures.lock().unwrap().insert("go env GOCACHE".into(), format!("{}\n", gocache.display()));

        let rr = scan(&f, "android.gradle_caches");
        assert_eq!(rr.items.len(), 1, "{:?}", rr.items.iter().map(|i| &i.path).collect::<Vec<_>>());
        assert!(rr.items[0].path.as_ref().unwrap().starts_with(gradle.to_str().unwrap()));
        assert_eq!(rr.locations[0].source, "$GRADLE_USER_HOME");
        let sys_imgs = scan(&f, "android.system_images");
        assert_eq!(sys_imgs.items.len(), 1);
        assert_eq!(sys_imgs.locations[0].source, "$ANDROID_HOME");
        let go = scan(&f, "go.build_cache");
        assert!(go.items[0].size >= 100_000, "measured at the tool's location");
        assert_eq!(go.locations[0].source, "`go env GOCACHE`");

        // and the items can be cleaned: the safety matcher uses the same resolved folders
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert!(plan.items[0].blocked.is_none(), "{:?}", plan.items[0].blocked);
        assert_eq!(run(&f, &plan, &RunOpts::default()).ok, 1);
        assert!(!gradle.join("caches/modules-2").exists());
    }

    #[test]
    fn a_variable_pointing_at_a_protected_folder_is_ignored_and_reported() {
        let f = fx();
        put(&f.home.join("Documents/caches/report/keep.txt"), 10_000);
        f.sys.env.lock().unwrap().insert("GRADLE_USER_HOME".into(), f.home.join("Documents").to_string_lossy().into_owned());
        let rr = scan(&f, "android.gradle_caches");
        assert!(rr.items.is_empty(), "nothing inside Documents is offered");
        assert!(rr.locations[0].skipped.iter().any(|s| s.contains("$GRADLE_USER_HOME")), "{:?}", rr.locations);
    }

    #[test]
    fn status_explains_each_rule_without_measuring() {
        let f = fx();
        put(&f.home.join("Library/Developer/Xcode/DerivedData/A-1/x"), 10);
        f.sys.missing_tools.lock().unwrap().insert("brew".into());
        f.sys.env.lock().unwrap().insert("GRADLE_USER_HOME".into(), f.home.join("Documents").to_string_lossy().into_owned());
        let all = f.eng.status_all();
        let by = |id: &str| all.iter().find(|s| s.id == id).unwrap().clone();
        assert_eq!(by("xcode.derived_data").state, "ready");
        assert_eq!(by("xcode.derived_data").matches, 1);
        assert_eq!(by("brew.cache").state, "not_installed");
        assert_eq!(by("android.gradle_caches").state, "location_ignored");
        assert_eq!(by("xcode.archives").state, "nothing_found", "no Archives folder in the fixture");
        assert_eq!(by("artifact.node_modules").state, "on_demand");
        f.sys.missing_apps.lock().unwrap().insert("org.mozilla.firefox".into());
        let reg = f.eng.registry();
        let ff = f.eng.rule_status(reg.get("browser.firefox").unwrap());
        assert_eq!((ff.state, ff.note.as_deref()), ("not_installed", Some("Firefox is not installed")));
        assert!(!scan(&f, "browser.firefox").available, "scans skip it too");
        assert_eq!(all.len(), f.eng.registry().rules.len());
    }

    #[test]
    fn custom_rules_are_tested_saved_and_cleaned_like_built_in_ones() {
        use crate::user_rules::Draft;
        let f = fx();
        put(&f.home.join("Downloads/a.dmg"), 300_000);
        put(&f.home.join("Downloads/b.dmg"), 100_000);
        put(&f.home.join("Downloads/keep.pdf"), 1_000);
        let d = Draft { id: "user.old_dmgs".into(), name: "Old DMGs".into(), category: "Custom".into(), paths: vec!["~/Downloads/*.dmg".into()], exclude: vec![], risk: Risk::Review, description: String::new(), busy: vec![] };
        let t = f.eng.test_rule(&d);
        assert!(t.ok, "{:?}", t.error);
        assert_eq!(t.count, 2);
        assert!(t.items[0].path.ends_with("a.dmg") && t.total >= 400_000);
        // a draft that would match protected things is refused
        fs::create_dir_all(f.home.join("Projects/app/.git")).unwrap();
        let bad = Draft { paths: vec!["~/Projects/*/.git".into()], ..d.clone() };
        let tb = f.eng.test_rule(&bad);
        assert!(!tb.ok && tb.blocked.len() == 1, "{tb:?}");
        // once added it scans, plans and cleans to the Trash through the normal path
        f.eng.set_custom_rules(&[d.clone()]);
        let rr = scan(&f, "user.old_dmgs");
        assert!(rr.rule.custom && rr.items.len() == 2);
        let plan = f.eng.plan(&[rr.clone()], &ids(&rr), &[], false);
        assert!(plan.items.iter().all(|i| i.method == Method::Trash && i.blocked.is_none()));
        assert_eq!(run(&f, &plan, &RunOpts::default()).ok, 2);
        assert!(f.home.join(".Trash/a.dmg").exists() && f.home.join("Downloads/keep.pdf").exists());
        // and removing it removes it
        f.eng.set_custom_rules(&[]);
        assert!(f.eng.registry().get("user.old_dmgs").is_none());
        assert!(f.eng.registry().get("xcode.derived_data").is_some(), "built-in rules stay");
    }

    #[test]
    fn scheduled_runs_only_clean_what_needs_no_confirmation_and_are_logged() {
        let f = fx();
        let dd = f.home.join("Library/Developer/Xcode/DerivedData");
        put(&dd.join("A-1/x"), 100_000);
        put(&dd.join("B-2/y"), 100_000);
        f.sys.open.lock().unwrap().push((777, "clang".into(), dd.join("B-2/y"))); // B-2 is in use
        let rep = f.eng.run_scheduled("xcode.derived_data", false, &HashSet::new()).unwrap();
        assert_eq!((rep.ok, rep.skipped), (1, 1), "the busy one waits for a person");
        assert!(!dd.join("A-1").exists() && dd.join("B-2/y").exists());
        let log = f.eng.undo.list();
        assert!(log.iter().all(|e| e.trigger == "scheduled"));
        assert_eq!(f.eng.undo.summary()["xcode.derived_data"].1, rep.trashed_bytes);
        // what may never be scheduled
        assert!(f.eng.run_scheduled("xcode.archives", true, &HashSet::new()).is_err(), "caution");
        assert!(f.eng.run_scheduled("appcache.library_caches", false, &HashSet::new()).is_err(), "review without consent");
        assert!(f.eng.schedule_problem("appcache.library_caches", true).is_none());
        assert!(f.eng.run_scheduled("brew.cache", true, &HashSet::new()).is_err(), "tool rules");
        assert!(f.eng.run_scheduled("system.trash", true, &HashSet::new()).is_err(), "emptying the Trash");
        let off: HashSet<String> = ["xcode.derived_data".to_string()].into();
        assert!(f.eng.run_scheduled("xcode.derived_data", false, &off).is_err(), "switched off");
    }
}
