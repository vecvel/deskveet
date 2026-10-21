// Thin typed wrapper over the Tauri IPC the backend exposes. No @tauri-apps/api: the global is injected.

declare global {
  interface Window {
    __TAURI__: { core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> } };
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

const call = <T>(cmd: string, args?: Record<string, unknown>) => window.__TAURI__.core.invoke<T>(cmd, args);

export const api = {
  overview: () => call<Overview>("overview"),
  processes: (query: { sort: "mem" | "cpu"; limit: number; query: string }) => call<ProcList>("processes", { query }),
  quit: (pid: number, start: number, force: boolean) => call<void>("quit_process", { pid, start, force }),
  reveal: (path: string) => call<void>("reveal", { path }),
  open: (path: string) => call<void>("open_path", { path }),
  quickLook: (path: string) => call<void>("quick_look", { path }),
  openFda: () => call<void>("open_fda_settings"),
};
