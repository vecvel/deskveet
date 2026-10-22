// Thin typed wrapper over the Tauri IPC the backend exposes. No @tauri-apps/api: the global is injected.

declare global {
  interface Window {
    __TAURI__: {
      core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
      event: { listen<T>(name: string, cb: (e: { payload: T }) => void): Promise<() => void> };
    };
  }
}

export type Pressure = "normal" | "warn" | "critical";

export interface DiskRow {
  name: string; mount: string; fs: string; total: number; used: number; free: number;
  purgeable: number; removable: boolean; used_pct: number;
}
export interface Mem {
  total: number; used: number; free: number; app: number; wired: number; compressed: number;
  cached: number; swap_used: number; swap_total: number; pressure: Pressure;
}
export interface Cpu { overall: number; cores: number[]; load: [number, number, number] }
export interface ProcRow {
  pid: number; name: string; user: string; mem: number; cpu: number; path: string | null;
  reveal: string | null; start: number; protected: string | null;
}
export interface Overview {
  host: string; home: string; uptime: number; cpu: Cpu; mem: Mem; disks: DiskRow[]; top_mem: ProcRow[]; top_cpu: ProcRow[];
}
export interface ProcList { rows: ProcRow[]; total: number }

export interface ScanMeta {
  v: number; root: string; scanned_at: number; duration_ms: number; files: number; bytes: number;
  dirs: number; denied_n: number; protected_n: number; denied: string[];
}
export interface ScanProgress { root: string; files: number; bytes: number; dirs: number; reused: number; current: string }
export interface ScanDone { root: string; cancelled: boolean; error: string | null; meta: ScanMeta | null }
export interface ScanStatus { home: string; running: ScanProgress | null; loaded: ScanMeta | null }
export type RowKind = "dir" | "file" | "small_dirs" | "small_files";
export interface Row { name: string; path: string; kind: RowKind; size: number; files: number; mtime: number }
export interface Children { path: string; size: number; files: number; rows: Row[] }

export type Risk = "safe" | "review" | "caution";
export type MethodName = "trash" | "tool" | "erase";
export interface RuleInfo {
  id: string; name: string; category: string; kind: string; risk: Risk; method: MethodName; description: string;
  regenerates: string; tool: string | null; opt_in: boolean; needs_admin: boolean; custom: boolean;
  paths: string[]; exclude: string[];
}
export interface Background { menubar: boolean; login: boolean; login_needs_approval: boolean; watched_rules: number }
export interface Point { t: number; b: number }
export interface Schedule { every: "daily" | "weekly"; hour: number; weekday: number; review_ok: boolean; created?: number }
export interface TrackInfo { points: Point[]; growth_per_day: number | null; last_cleaned: number | null; freed: number; alert: number | null; schedule: Schedule | null; schedule_problem: string | null }
export interface Tracking { rules: Record<string, TrackInfo>; disk_free_pct: number | null }
export interface Draft { id: string; name: string; category: string; paths: string[]; exclude: string[]; risk: Risk; description: string; busy: string[] }
export interface TestItem { path: string; size: number; why: string | null }
export interface RuleTest { ok: boolean; error: string | null; count: number; total: number; measured: number; items: TestItem[]; blocked: TestItem[] }
export interface CItem {
  id: string; rule_id: string; label: string; path: string | null; size: number; files: number; mtime: number;
  blocked: string | null; info: string[];
}
export interface Location { id: string; name: string; path: string | null; source: string; skipped: string[] }
export type RuleState = "ready" | "not_installed" | "nothing_found" | "location_ignored" | "needs_fda" | "on_demand";
export interface RuleStatus { id: string; state: RuleState; note: string | null; locations: Location[]; matches: number }
export interface RuleResult { rule: RuleInfo; locations: Location[]; available: boolean; note: string | null; items: CItem[]; total: number; running: string[] }
export interface CleanupState { running: boolean; rules_version: string; results: RuleResult[]; undo_log: string }
export interface PlanItem {
  id: string; rule_id: string; rule_name: string; label: string; path: string | null; size: number; method: MethodName;
  risk: Risk; command: string | null; recoverable: boolean; blocked: string | null; confirm: string[]; admin_cmd: string | null;
}
export interface Plan { token: string; dry_run: boolean; items: PlanItem[]; total: number; erase: boolean }
export interface ItemResult {
  id: string; label: string; path: string | null; status: "ok" | "failed" | "skipped" | "dry-run"; method: MethodName;
  bytes: number; trash_path: string | null; error: string | null;
}
export interface Report {
  run: string; dry_run: boolean; results: ItemResult[]; trashed_bytes: number; freed_bytes: number;
  ok: number; failed: number; skipped: number; undo_log: string;
}
export interface UndoEntry {
  id: string; run: string; ts: number; at: string; rule: string; label: string; action: string; status: string;
  path: string | null; trash_path: string | null; bytes: number; command: string[] | null; error: string | null; restored: boolean;
}

export interface ArtItem { id: string; rule_id: string; rule_name: string; dir: string; path: string; size: number; files: number; blocked: string | null }
export interface Project { path: string; name: string; last_active: number; items: ArtItem[]; total: number; running: string[] }
export interface ArtifactScan { roots: string[]; projects: Project[]; total: number; dirs: number }
export interface ArtifactState { busy: boolean; default_roots: string[]; scan: ArtifactScan | null }

export interface About { version: string; rules_version: string; rules_count: number; cache_dir: string; cache_bytes: number; data_dir: string; undo_log: string }

export interface AppRow {
  pid: number; name: string; bundle_id: string; path: string | null; mem: number; cpu: number; procs: number;
  bundle_bytes: number | null; data_bytes: number | null; protected: string | null; icon: string | null;
}
export interface QuitResult { pid: number; ok: boolean; error: string | null }

const call = <T>(cmd: string, args?: Record<string, unknown>) => window.__TAURI__.core.invoke<T>(cmd, args);

export const api = {
  overview: () => call<Overview>("overview"),
  processes: (query: { sort: "mem" | "cpu"; limit: number; query: string }) => call<ProcList>("processes", { query }),
  quit: (pid: number, start: number, force: boolean) => call<void>("quit_process", { pid, start, force }),
  reveal: (path: string) => call<void>("reveal", { path }),
  open: (path: string) => call<void>("open_path", { path }),
  quickLook: (path: string) => call<void>("quick_look", { path }),
  openFda: () => call<void>("open_fda_settings"),
  pickFolder: () => call<string | null>("pick_folder"),
  scanStart: (root: string, full: boolean) => call<void>("scan_start", { root, full }),
  scanCancel: () => call<void>("scan_cancel"),
  scanStatus: () => call<ScanStatus>("scan_status"),
  scanLoadCached: (root: string) => call<ScanMeta | null>("scan_load_cached", { root }),
  scanChildren: (path: string) => call<Children>("scan_children", { path }),
  cleanupState: () => call<CleanupState>("cleanup_state"),
  cleanupScan: () => call<void>("cleanup_scan"),
  cleanupCancel: () => call<void>("cleanup_cancel"),
  cleanupPlan: (req: { ids: string[]; picks: string[]; dry_run: boolean }) => call<Plan>("cleanup_plan", { req }),
  cleanupRun: (req: { token: string; confirm_erase: boolean; overrides: string[] }) => call<Report>("cleanup_run", { req }),
  undoList: () => call<UndoEntry[]>("undo_list"),
  undoRestore: (id: string) => call<string>("undo_restore", { id }),
  undoClear: () => call<void>("undo_clear"),
  openTrash: () => call<void>("open_trash"),
  artifactState: () => call<ArtifactState>("artifact_state"),
  artifactScan: (roots: string[]) => call<void>("artifact_scan", { roots }),
  about: () => call<About>("about"),
  appsList: () => call<AppRow[]>("apps_list"),
  appsQuit: (targets: { pid: number; bundle_id: string }[], force: boolean) => call<QuitResult[]>("apps_quit", { targets, force }),
  rulesList: () => call<RuleInfo[]>("rules_list"),
  rulesStatus: () => call<RuleStatus[]>("rules_status"),
  rulesCustom: () => call<Draft[]>("rules_custom"),
  trackingGet: () => call<Tracking>("tracking_get"),
  alertSet: (ruleId: string, bytes: number | null) => call<void>("alert_set", { ruleId, bytes }),
  diskAlertSet: (pct: number | null) => call<void>("disk_alert_set", { pct }),
  scheduleSet: (ruleId: string, schedule: Schedule | null) => call<void>("schedule_set", { ruleId, schedule }),
  notifyTest: () => call<void>("notify_test"),
  uiVisible: (visible: boolean) => call<void>("ui_visible", { visible }),
  backgroundGet: () => call<Background>("background_get"),
  backgroundSet: (opts: { menubar?: boolean; login?: boolean }) => call<Background>("background_set", opts),
  ruleTest: (draft: Draft) => call<RuleTest>("rule_test", { draft }),
  ruleSave: (draft: Draft) => call<string>("rule_save", { draft }),
  ruleDelete: (id: string) => call<void>("rule_delete", { id }),
  rulesExport: () => call<string | null>("rules_export"),
  rulesImport: () => call<{ added: number; skipped: string[] } | null>("rules_import"),
  locationsInfo: () => call<Location[]>("locations_info"),
  locationsRedetect: () => call<Location[]>("locations_redetect"),
  fdaStatus: () => call<"granted" | "denied" | "unknown">("fda_status"),
  clearScanCache: () => call<number>("scan_clear_cache"),
  listen: <T>(name: string, cb: (payload: T) => void) => window.__TAURI__.event.listen<T>(name, (e) => cb(e.payload)),
};
