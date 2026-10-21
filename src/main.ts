import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/components.css";
import { h } from "./dom";
import { api } from "./api";
import { env } from "./fmt";
import { store } from "./store";
import { cleanupView } from "./views/cleanup";
import { overviewView } from "./views/overview";
import { processesView } from "./views/processes";
import { applyTheme, releaseSeconds, settingsView, Theme } from "./views/settings";
import { storageView } from "./views/storage";
import { View } from "./views/types";

// Outline icons in the style of SF Symbols: 24px grid, 1.6px rounded strokes, one per destination.
const ICONS: Record<string, string> = {
  // square.grid.2x2
  overview: '<rect x="3.5" y="3.5" width="7" height="7" rx="2"/><rect x="13.5" y="3.5" width="7" height="7" rx="2"/><rect x="3.5" y="13.5" width="7" height="7" rx="2"/><rect x="13.5" y="13.5" width="7" height="7" rx="2"/>',
  // internaldrive
  storage: '<rect x="2.5" y="7" width="19" height="10" rx="3"/><path d="M2.8 12.2h18.4"/><circle cx="17.4" cy="14.6" r=".9" fill="currentColor" stroke="none"/><path d="M5.5 14.6h4"/>',
  // sparkles
  cleanup: '<path d="M10 3.5c.6 4.2 2.3 5.9 6.5 6.5-4.2.6-5.9 2.3-6.5 6.5-.6-4.2-2.3-5.9-6.5-6.5 4.2-.6 5.9-2.3 6.5-6.5z"/><path d="M17.5 14c.3 2.1 1.1 2.9 3.2 3.2-2.1.3-2.9 1.1-3.2 3.2-.3-2.1-1.1-2.9-3.2-3.2 2.1-.3 2.9-1.1 3.2-3.2z"/>',
  // cpu
  processes: '<rect x="6" y="6" width="12" height="12" rx="2.5"/><rect x="9.5" y="9.5" width="5" height="5" rx="1"/><path d="M9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3"/>',
  // gearshape
  settings: '<path d="M18.62 9.43 L21.00 10.08 L21.00 13.92 L18.62 14.57 L18.50 14.87 L19.72 17.01 L17.01 19.72 L14.87 18.50 L14.57 18.62 L13.92 21.00 L10.08 21.00 L9.43 18.62 L9.13 18.50 L6.99 19.72 L4.28 17.01 L5.50 14.87 L5.38 14.57 L3.00 13.92 L3.00 10.08 L5.38 9.43 L5.50 9.13 L4.28 6.99 L6.99 4.28 L9.13 5.50 L9.43 5.38 L10.08 3.00 L13.92 3.00 L14.57 5.38 L14.87 5.50 L17.01 4.28 L19.72 6.99 L18.50 9.13 Z"/><circle cx="12" cy="12" r="3"/>',
};

const TABS = [
  { id: "overview", label: "Overview" },
  { id: "storage", label: "Storage" },
  { id: "cleanup", label: "Cleanup" },
  { id: "processes", label: "Processes" },
  { id: "settings", label: "Settings" },
] as const;

try { void api.scanStatus().then((s) => { env.home = s.home; }).catch(() => {}); } catch { /* backend not ready */ }

const main = document.getElementById("main")!;
const nav = document.getElementById("nav")!;
const views = new Map<string, View>();
let current = "";
let timer = 0;

function make(id: string): View {
  switch (id) {
    case "overview": return overviewView(show);
    case "processes": return processesView();
    case "storage": return storageView();
    case "cleanup": return cleanupView();
    default: return settingsView();
  }
}

function show(id: string) {
  if (id === current) return;
  clearTimeout(timer);
  views.get(current)?.unmount?.();
  current = id;
  let v = views.get(id);
  main.replaceChildren();
  if (!v) { v = make(id); views.set(id, v); }
  v.mount(main);
  main.scrollTop = 0;
  nav.querySelectorAll("button").forEach((b) => (b.dataset.id === id ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current")));
  store.set("tab", id);
  void loop(true);
}

/** Live refresh only while the window is visible and the active view wants it. No work when hidden. */
async function loop(now = false) {
  clearTimeout(timer);
  const v = views.get(current);
  if (!v?.tick || document.hidden) return;
  const id = current;
  if (now) { try { await v.tick(); } catch (e) { console.error(e); } }
  if (id !== current) return;
  timer = window.setTimeout(() => void loop(true), v.interval ?? 2000);
}

// After a minute in the background the window is released: the webview and its three WebKit helper
// processes (~70 MB) go away, the Rust side keeps running. Dock click / Cmd+Tab brings the window back.
let releaseTimer = 0;
document.addEventListener("visibilitychange", () => {
  clearTimeout(releaseTimer);
  if (document.hidden) {
    clearTimeout(timer);
    const secs = releaseSeconds();
    if (secs > 0) releaseTimer = window.setTimeout(() => void window.__TAURI__.core.invoke("release_window"), secs * 1000);
  } else void loop(true);
});

const GROUPS: [string, string[]][] = [["Monitor", ["overview"]], ["Disk", ["storage", "cleanup"]], ["System", ["processes", "settings"]]];
for (const [label, ids] of GROUPS) {
  nav.append(h("div", { class: "nav-label", text: label }));
  for (const id of ids) {
    const i = TABS.findIndex((t) => t.id === id);
    const b = h("button", { "data-id": id, title: TABS[i].label, "aria-label": TABS[i].label, onclick: () => show(id) });
    b.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[id]}</svg><span class="lbl">${TABS[i].label}</span><kbd>⌘${i + 1}</kbd>`;
    nav.append(b);
  }
}

addEventListener("keydown", (e) => {
  if (!e.metaKey && !e.ctrlKey) return;
  const n = Number(e.key);
  if (n >= 1 && n <= TABS.length) { e.preventDefault(); show(TABS[n - 1].id); }
  else if (e.key === "f") { e.preventDefault(); views.get(current)?.find?.(); }
  else if (e.key === "r") { e.preventDefault(); views.get(current)?.refresh?.(e.shiftKey); }
});
addEventListener("contextmenu", (e) => e.preventDefault());

void store.load().then(() => {
  applyTheme(store.get<Theme>("theme", "system"));
  const last = store.get("tab", "overview");
  show(TABS.some((t) => t.id === last) ? last : "overview");
});
