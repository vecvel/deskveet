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
  listen: <T>(name: string, cb: (payload: T) => void) => window.__TAURI__.event.listen<T>(name, (e) => cb(e.payload)),
};
