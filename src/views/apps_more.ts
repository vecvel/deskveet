// The Apps view's other tabs: installed apps (uninstall), data left by deleted apps, and startup items.
// Removing always goes through the normal preview and the Trash.
import { api, DataItem, InstalledApp, LeftoverGroup, StartupItem } from "../api";
import { h, openDialog, toast } from "../dom";
import { bytes, date, tilde } from "../fmt";
import { icons } from "../icons";
import { openPreview } from "../preview";

export interface Pane {
  el: HTMLElement;
  /** Fetch and paint; called when the tab is shown. */
  load(): Promise<void>;
  filter(q: string): void;
  /** Called on the view's timer; only some panes poll. */
  tick?(): Promise<void>;
}

const ibtn = (icon: string, label: string, fn: () => void) => {
  const el = h("button", { class: "ibtn", title: label, "aria-label": label, onclick: fn });
  el.innerHTML = icon;
  return el;
};
const empty = (cols: number, text: string) => h("tr", {}, h("td", { colspan: cols, class: "empty", text }));
const reveal = (p: string) => api.reveal(p).catch((e) => toast(String(e), true));

// ── Installed: list apps, uninstall one with its data ──

export function installedPane(): Pane {
  let rows: InstalledApp[] = [];
  let query = "";
  const body = h("tbody");
  const el = h("div", { class: "tbl-wrap" }, h("table", { class: "tbl ia" },
    h("colgroup", {}, h("col"), h("col"), h("col"), h("col")),
    h("thead", {}, h("tr", {}, h("th", { text: "App" }), h("th", { class: "r", text: "Storage" }), h("th", { class: "r", text: "Last opened" }), h("th", {}))),
    body));

  function paint() {
    const q = query.trim().toLowerCase();
    const list = rows.filter((r) => !q || r.name.toLowerCase().includes(q) || r.bundle_id.toLowerCase().includes(q));
    if (!list.length) { body.replaceChildren(empty(4, rows.length ? "No app matches" : "Loading…")); return; }
    body.replaceChildren(...list.map((r) => {
      const storage = r.bundle_bytes == null
        ? h("td", { class: "r tertiary", text: "Measuring…" })
        : h("td", { class: "r", title: `App ${bytes(r.bundle_bytes)} · data ${bytes(r.data_bytes ?? 0)}` },
            h("div", { class: "sz", text: bytes(r.bundle_bytes + (r.data_bytes ?? 0)) }), h("div", { class: "tertiary small", text: `data ${bytes(r.data_bytes ?? 0)}` }));
      const action = r.locked
        ? h("span", { class: "lock", text: r.locked })
        : r.running
          ? h("span", { class: "lock", text: "Open", title: "Quit it to uninstall" })
          : h("button", { class: "btn sm", text: "Uninstall…", onclick: () => void uninstall(r) });
      return h("tr", {},
        h("td", {}, h("div", { class: "pv-main" },
          h("div", { class: "nm", text: r.version ? `${r.name}  ` : r.name }, r.version ? h("span", { class: "tertiary small", text: r.version }) : null),
          h("div", { class: "path", title: r.path, text: tilde(r.path) }))),
        storage,
        h("td", { class: "r muted", text: r.last_used ? date(r.last_used) : "Not known" }),
        h("td", { class: "act" }, ibtn(icons.reveal, "Reveal in Finder", () => void reveal(r.path)), action));
    }));
  }

  /** The app and its data, all ticked; the ticked ones go to the preview and then the Trash. */
  async function uninstall(r: InstalledApp) {
    let items: DataItem[];
    try { items = await api.appUninstallItems(r.path); } catch (e) { toast(String(e), true); return; }
    const { dlg, done } = openDialog("wide", "un-title");
    const ticked = new Set(items.map((i) => i.path));
    const total = h("span", { class: "muted" });
    const go = h("button", { class: "btn primary", text: "Review and move to Trash…" });
    const paintTotal = () => {
      const sum = items.filter((i) => ticked.has(i.path)).reduce((s, i) => s + i.size, 0);
      total.textContent = `${ticked.size} of ${items.length} selected · ${bytes(sum)}`;
      go.toggleAttribute("disabled", ticked.size === 0);
    };
    const list = h("div", { class: "pv-list" }, ...items.map((i) => {
      const cb = h("input", { type: "checkbox", "aria-label": `Include ${tilde(i.path)}` });
      cb.checked = true;
      cb.addEventListener("change", () => { cb.checked ? ticked.add(i.path) : ticked.delete(i.path); paintTotal(); });
      return h("label", { class: "pv-row" }, cb,
        h("div", { class: "pv-main" }, h("div", { class: "row gap-2" }, h("b", { text: i.kind })),
          h("div", { class: "path", title: i.path, text: tilde(i.path) })),
        h("div", { class: "sz", text: bytes(i.size) }));
    }));
    go.addEventListener("click", () => {
      const picks = items.filter((i) => ticked.has(i.path)).map((i) => i.path);
      done();
      void openPreview({ picks, dryRun: false, onCleaned: () => { toast(`${r.name} moved to the Trash`); void load(); } });
    });
    dlg.append(
      h("h3", { id: "un-title", text: `Uninstall ${r.name}` }),
      h("p", { class: "muted", text: r.needs_admin
        ? "Everything you leave ticked moves to the Trash. macOS will ask for your password to move this app itself, because it belongs to the system."
        : "Everything you leave ticked moves to the Trash, so you can put it back. Untick anything you want to keep, such as settings you may need later." }),
      h("div", { class: "pv-body" }, list),
      h("div", { class: "btns" }, total, h("span", { class: "sp" }), h("button", { class: "btn", text: "Cancel", onclick: done }), go));
    paintTotal();
  }

  async function load() { rows = await api.appsInstalled(); paint(); }
  return {
    el, load, filter(q) { query = q; paint(); },
    // sizes are measured in the background; poll until they are all in
    async tick() { if (rows.some((r) => r.bundle_bytes == null)) await load(); },
  };
}

// ── Leftovers: data of apps that are gone ──

export function leftoversPane(): Pane {
  let groups: LeftoverGroup[] = [];
  let query = "";
  let loaded = false;
  const sub = h("p", { class: "muted small", text: "Data in your Library named after apps that are no longer on this Mac and untouched for a week. Apple's own data is never listed." });
  const box = h("div", { class: "stack" });
  const el = h("div", { class: "stack" }, sub, box);

  function paint() {
    const q = query.trim().toLowerCase();
    const list = groups.filter((g) => !q || g.id.includes(q) || g.items.some((i) => i.path.toLowerCase().includes(q)));
    if (!loaded) { box.replaceChildren(h("p", { class: "empty", text: "Looking through your Library…" })); return; }
    if (!list.length) { box.replaceChildren(h("p", { class: "empty", text: groups.length ? "Nothing matches" : "No leftovers found. Every app's data has its app." })); return; }
    const total = list.reduce((s, g) => s + g.total, 0);
    const all = h("button", { class: "btn primary", text: `Review all (${bytes(total)})…`, onclick: () => clean(list.flatMap((g) => g.items.map((i) => i.path))) });
    box.replaceChildren(
      h("div", { class: "toolbar" }, h("span", { class: "muted", text: `${list.length} app${list.length === 1 ? "" : "s"} · ${bytes(total)}` }), h("span", { class: "sp" }), all),
      h("div", { class: "card flush" }, ...list.map((g) => h("div", { class: "rule-row lo" },
        h("div", { class: "pv-main" },
          h("div", { class: "nm", text: g.id }),
          h("div", { class: "tertiary small", text: `${[...new Set(g.items.map((i) => i.kind))].join(", ")} · last changed ${date(g.newest)}` })),
        h("span", { class: "sz", text: bytes(g.total) }),
        h("div", { class: "row gap-2" },
          ibtn(icons.reveal, "Reveal in Finder", () => void reveal(g.items[0].path)),
          h("button", { class: "btn sm", text: "Move to Trash…", onclick: () => clean(g.items.map((i) => i.path)) }))))));
  }

  function clean(picks: string[]) {
    void openPreview({ picks, dryRun: false, onCleaned: () => { toast("Moved to the Trash"); void load(); } });
  }

  async function load() {
    paint();
    try { groups = await api.appLeftovers(); } catch (e) { toast(String(e), true); }
    loaded = true;
    paint();
  }
  return { el, load, filter(q) { query = q; paint(); } };
}

// ── Startup: what starts with the Mac (read-only) ──

export function startupPane(): Pane {
  let rows: StartupItem[] = [];
  let query = "";
  const body = h("tbody");
  const el = h("div", { class: "stack" },
    h("div", { class: "toolbar" },
      h("p", { class: "muted small", text: "Helpers that start with your Mac. Turn them off in System Settings, where macOS lets you. Apps set to open at login are listed there too." }),
      h("span", { class: "sp" }),
      h("button", { class: "btn", text: "Open Login Items settings", onclick: () => void api.openLoginItems().catch((e) => toast(String(e), true)) })),
    h("div", { class: "tbl-wrap" }, h("table", { class: "tbl su" },
      h("colgroup", {}, h("col"), h("col"), h("col"), h("col")),
      h("thead", {}, h("tr", {}, h("th", { text: "Item" }), h("th", { text: "For" }), h("th", { text: "Status" }), h("th", {}))),
      body)));

  function paint() {
    const q = query.trim().toLowerCase();
    const list = rows.filter((r) => !q || r.owner.toLowerCase().includes(q) || r.label.toLowerCase().includes(q));
    if (!list.length) { body.replaceChildren(empty(4, rows.length ? "Nothing matches" : "No startup items found")); return; }
    body.replaceChildren(...list.map((r) => {
      const how = [r.run_at_load ? "starts at login" : "", r.keep_alive ? "kept running" : "", r.disabled ? "turned off" : ""].filter(Boolean).join(", ");
      return h("tr", {},
        h("td", {}, h("div", { class: "pv-main" },
          h("div", { class: "nm", text: r.owner }),
          h("div", { class: "path", title: r.program ?? r.label, text: r.program ? tilde(r.program) : r.label }))),
        h("td", { class: "muted", text: r.scope }),
        h("td", {}, h("span", { class: r.running ? "ok-text" : "muted", text: r.running ? "Running" : "Not running" }),
          how ? h("div", { class: "tertiary small", text: how }) : null),
        h("td", { class: "act" }, ibtn(icons.reveal, "Reveal its file in Finder", () => void reveal(r.plist))));
    }));
  }

  async function load() { rows = await api.startupItems(); paint(); }
  return { el, load, filter(q) { query = q; paint(); } };
}
