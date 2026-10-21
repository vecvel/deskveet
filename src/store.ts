/** Per-viewer conveniences (last tab, folder, view mode). Never required for correctness. */
export const store = {
  get<T>(key: string, fallback: T): T {
    try {
      const v = localStorage.getItem("dv." + key);
      return v == null ? fallback : (JSON.parse(v) as T);
    } catch { return fallback; }
  },
  set(key: string, value: unknown) {
    try { localStorage.setItem("dv." + key, JSON.stringify(value)); } catch { /* private mode etc. */ }
  },
};
