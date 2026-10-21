import { h } from "./dom";
import { overviewView } from "./views/overview";
import { processesView } from "./views/processes";
import { soonView } from "./views/soon";
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

const main = document.getElementById("main")!;
const nav = document.getElementById("nav")!;
const views = new Map<string, View>();
let current = "";
let timer = 0;

function make(id: string): View {
  switch (id) {
    case "overview": return overviewView(show);
    case "processes": return processesView();
    case "storage": return soonView("Storage", "Parallel scan, sortable folder sizes and a treemap.", "step 2");
    case "cleanup": return soonView("Cleanup", "Rule-based cleanup with preview, Trash and an undo log.", "step 3");
    default: return soonView("Settings", "Theme, scan roots and the undo log.", "step 5");
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
  nav.querySelectorAll("button").forEach((b) => b.toggleAttribute("aria-current", b.dataset.id === id));
  nav.querySelectorAll("button").forEach((b) => (b.dataset.id === id ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current")));
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
const RELEASE_AFTER_MS = 60_000;
let releaseTimer = 0;
document.addEventListener("visibilitychange", () => {
  clearTimeout(releaseTimer);
  if (document.hidden) {
    clearTimeout(timer);
    releaseTimer = window.setTimeout(() => void window.__TAURI__.core.invoke("release_window"), RELEASE_AFTER_MS);
  } else void loop(true);
});

nav.append(
  ...TABS.map((t, i) => {
    const b = h("button", { "data-id": t.id, onclick: () => show(t.id) });
    b.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[t.id]}</svg>${t.label}<kbd>⌘${i + 1}</kbd>`;
    return b;
  }),
);

addEventListener("keydown", (e) => {
  if (!e.metaKey && !e.ctrlKey) return;
  const n = Number(e.key);
  if (n >= 1 && n <= TABS.length) { e.preventDefault(); show(TABS[n - 1].id); }
  else if (e.key === "f") { e.preventDefault(); views.get(current)?.find?.(); }
  else if (e.key === "r") { e.preventDefault(); views.get(current)?.refresh?.(); }
});
addEventListener("contextmenu", (e) => e.preventDefault());

show("overview");
