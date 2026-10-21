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

const ICONS: Record<string, string> = {
  overview: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  storage: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  cleanup: '<path d="M4 20l7-7M14 4l6 6-8 8-6-6zM17 7l-3 3"/>',
  processes: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  settings: '<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M19.1 4.9L17 7M7 17l-2.1 2.1"/>',
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
    b.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[id]}</svg><span class="lbl">${TABS[i].label}</span><kbd>⌘${i + 1}</kbd>`;
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
