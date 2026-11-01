/** Per-user UI preferences (last tab, folders, view modes). Never required for correctness.
 *  The source of truth is the Rust process (it outlives the window); localStorage is only a fallback. */
const cache: Record<string, unknown> = {};

export const store = {
  /** Call once before the first view is created. */
  async load() {
    try {
      Object.assign(cache, await window.__TAURI__.core.invoke<Record<string, unknown>>("ui_get_all"));
      return;
    } catch { /* backend unavailable: fall back to localStorage */ }
    try {
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i);
        if (k?.startsWith("dv.")) cache[k.slice(3)] = JSON.parse(localStorage.getItem(k)!);
      }
    } catch { /* private mode etc. */ }
  },
  get<T>(key: string, fallback: T): T {
    return key in cache ? (cache[key] as T) : fallback;
  },
  set(key: string, value: unknown) {
    cache[key] = value;
    try { void window.__TAURI__.core.invoke("ui_set", { key, value }).catch(() => {}); } catch { /* no backend */ }
    try { localStorage.setItem("dv." + key, JSON.stringify(value)); } catch { /* ignore */ }
  },
};
