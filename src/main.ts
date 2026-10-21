import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/components.css";
import { h } from "./dom";
import { api } from "./api";
import { env } from "./fmt";
import { autoHideScrollbars } from "./scrollbars";
import { store } from "./store";
import { appsView } from "./views/apps";
import { cleanupView } from "./views/cleanup";
import { overviewView } from "./views/overview";
import { processesView } from "./views/processes";
import { applyTheme, releaseSeconds, settingsView, Theme } from "./views/settings";
import { storageView } from "./views/storage";
import { View } from "./views/types";

// Sidebar glyphs (24px grid). Drawn on a warm tile: tinted when idle, solid when the tab is active.
const ICONS: Record<string, string> = {
  overview: '<path d="M4.5 16.5a8 8 0 1 1 15 0"/><path d="M12 13.5l3.6-4.2"/><circle cx="12" cy="13.5" r="1.3" class="dot"/><path d="M6.5 19.5h11"/>', // gauge
  storage: '<circle cx="12" cy="12" r="8.5"/><path d="M12 3.5V12h8.5"/>', // pie chart
  cleanup: '<path d="M16.5 3l-4.2 7.5"/><path d="M9.3 10.5h6l2.5 9.5H6.5z"/><path d="M9.6 20l1.2-5M12.6 20l.3-5M15.6 20l-.6-5"/>', // broom
  processes: '<path d="M3 12h3.5l2.5-6 4 12 2.5-6H21"/>', // activity
  apps: '<path d="M12 3.5v7.5"/><path d="M7.2 6.6a7.5 7.5 0 1 0 9.6 0"/>', // power: quit apps
  settings: '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>', // sliders
};

const TABS = [
  { id: "overview", label: "Overview" },
  { id: "storage", label: "Storage" },
  { id: "cleanup", label: "Cleanup" },
  { id: "apps", label: "Apps" },
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
    case "apps": return appsView();
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
try { void api.uiVisible(true).catch(() => {}); } catch { /* backend not ready */ }
document.addEventListener("visibilitychange", () => {
  try { void api.uiVisible(!document.hidden).catch(() => {}); } catch { /* backend not ready */ }
  clearTimeout(releaseTimer);
  if (document.hidden) {
    clearTimeout(timer);
    const secs = releaseSeconds();
    if (secs > 0) releaseTimer = window.setTimeout(() => void window.__TAURI__.core.invoke("release_window"), secs * 1000);
  } else void loop(true);
});

const GROUPS: [string, string[]][] = [["Monitor", ["overview"]], ["Disk", ["storage", "cleanup"]], ["System", ["apps", "processes", "settings"]]];
for (const [label, ids] of GROUPS) {
  nav.append(h("div", { class: "nav-label", text: label }));
  for (const id of ids) {
    const i = TABS.findIndex((t) => t.id === id);
    const b = h("button", { "data-id": id, title: TABS[i].label, "aria-label": TABS[i].label, onclick: () => show(id) });
    b.innerHTML = `<svg class="tile t-${id}" viewBox="0 0 24 24" aria-hidden="true"><rect x="1" y="1" width="22" height="22" rx="6"/><g transform="translate(4.6 4.6) scale(.617)">${ICONS[id]}</g></svg><span class="lbl">${TABS[i].label}</span><kbd>⌘${i + 1}</kbd>`;
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
autoHideScrollbars();
addEventListener("contextmenu", (e) => e.preventDefault());

void store.load().then(() => {
  applyTheme(store.get<Theme>("theme", "system"));
  const last = store.get("tab", "overview");
  show(TABS.some((t) => t.id === last) ? last : "overview");
});
