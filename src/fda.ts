import { api } from "./api";

export type FdaStatus = "granted" | "denied" | "unknown";
let status: FdaStatus = "unknown";
const subs = new Set<() => void>();

export const fda = {
  get: () => status,
  /** Re-check now; listeners re-render when the answer changes. */
  async refresh() {
    try {
      const s = await api.fdaStatus();
      if (s !== status) { status = s; subs.forEach((f) => f()); }
    } catch { /* backend not ready */ }
  },
  subscribe(f: () => void) { subs.add(f); return () => subs.delete(f); },
};

// The user grants access in System Settings and comes back: check again.
addEventListener("focus", () => void fda.refresh());
document.addEventListener("visibilitychange", () => { if (!document.hidden) void fda.refresh(); });

export const FDA_HELP =
  "Turn on Full Disk Access for Deskveet in System Settings. If it is already on, remove Deskveet from the list and add it again: macOS ties the permission to each build of the app.";
