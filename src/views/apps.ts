import { api, AppRow } from "../api";
import { confirmDialog, h, modal, toast } from "../dom";
import { bytes, num, pct, ram, tilde } from "../fmt";
import { header, View } from "./types";
import { installedPane, leftoversPane, Pane, startupPane } from "./apps_more";
import { store } from "../store";

type Tab = "running" | "installed" | "leftovers" | "startup";

type Key = "name" | "mem" | "cpu" | "disk";

export function appsView(): View {
  let rows: AppRow[] = [];
  let key: Key = "mem";
  let dir: 1 | -1 = -1;
  let query = "";
  const selected = new Set<number>();
  let body!: HTMLElement, search!: HTMLInputElement, sub!: HTMLElement, bulk!: HTMLElement;
  let ths: Partial<Record<Key, HTMLElement>> = {};

  const disk = (r: AppRow) => (r.bundle_bytes ?? 0) + (r.data_bytes ?? 0);
  const cmp: Record<Key, (a: AppRow, b: AppRow) => number> = {
    name: (a, b) => a.name.localeCompare(b.name),
    mem: (a, b) => a.mem - b.mem,
    cpu: (a, b) => a.cpu - b.cpu,
    disk: (a, b) => disk(a) - disk(b),
  };

  async function quit(targets: AppRow[], force: boolean) {
    const ok = targets.filter((t) => !t.protected);
    if (!ok.length) return;
    const names = ok.length === 1 ? `\u201c${ok[0].name}\u201d` : `${ok.length} apps`;
    // A normal quit is what Cmd+Q does: the app can still ask to save. Ask first only for several apps or a force quit.
    if (force || ok.length > 1) {
      const yes = await confirmDialog({
        title: `${force ? "Force quit" : "Quit"} ${names}?`,
        body: force ? "They are stopped immediately. Unsaved work in them is lost." : "Each app is asked to quit and can still ask you to save its work.",
        ok: force ? "Force quit" : "Quit", danger: force,
      });
      if (!yes) return;
    }
    const res = await api.appsQuit(ok.map((t) => ({ pid: t.pid, bundle_id: t.bundle_id })), force);
    const failed = res.filter((r) => !r.ok);
    for (const r of res) if (r.ok) selected.delete(r.pid);
    if (failed.length) toast(failed.map((f) => f.error).join(" "), true);
    else toast(force ? `Force quit ${names}` : `Asked ${names} to quit`);
    setTimeout(() => void view.tick!(), 700);
  }

  function paintBulk() {
    const sel = rows.filter((r) => selected.has(r.pid));
    bulk.replaceChildren();
    if (!sel.length) return;
    bulk.append(
      h("span", { class: "muted", text: `${sel.length} selected \u00b7 ${ram(sel.reduce((s, r) => s + r.mem, 0))}` }),
      h("button", { class: "btn sm", text: "Quit", onclick: () => void quit(sel, false) }),
      h("button", { class: "btn sm", text: "Force quit", onclick: () => void quit(sel, true) }));
  }

  function paint() {
    const q = query.trim().toLowerCase();
    const list = rows.filter((r) => !q || r.name.toLowerCase().includes(q) || r.bundle_id.toLowerCase().includes(q)).sort((a, b) => dir * cmp[key](a, b));
    const total = rows.reduce((s, r) => s + r.mem, 0);
    sub.textContent = `${num(rows.length)} running \u00b7 ${ram(total)} of memory`;
    for (const [k, th] of Object.entries(ths)) k === key ? th!.setAttribute("aria-sort", dir === 1 ? "ascending" : "descending") : th!.removeAttribute("aria-sort");
    body.replaceChildren(...list.map((r) => {
      const cb = h("input", { type: "checkbox", "aria-label": `Select ${r.name}`, disabled: !!r.protected });
      cb.checked = selected.has(r.pid);
      cb.addEventListener("change", () => { cb.checked ? selected.add(r.pid) : selected.delete(r.pid); paintBulk(); });
      const icon = r.icon ? h("img", { class: "app-ico", src: r.icon, alt: "" }) : h("span", { class: "app-ico none", text: r.name.slice(0, 1) });
      const storage = r.bundle_bytes == null
        ? h("td", { class: "r tertiary", text: "Measuring\u2026" })
        : h("td", { class: "r", title: `App ${bytes(r.bundle_bytes)} \u00b7 caches and data ${bytes(r.data_bytes ?? 0)}` },
            h("div", { class: "sz", text: bytes(disk(r)) }), h("div", { class: "tertiary small", text: `data ${bytes(r.data_bytes ?? 0)}` }));
      return h("tr", {},
        h("td", { class: "ck" }, cb),
        h("td", {}, h("div", { class: "row gap-2" }, icon,
          h("div", { class: "pv-main" }, h("div", { class: "nm", text: r.name }), h("div", { class: "path", title: r.path ?? "", text: r.path ? tilde(r.path) : r.bundle_id })))),
        h("td", { class: "r" }, h("div", { class: "sz", text: ram(r.mem) }), h("div", { class: "tertiary small", text: `${r.procs} process${r.procs === 1 ? "" : "es"}` })),
        h("td", { class: "r", text: pct(r.cpu, r.cpu >= 100 ? 0 : 1) }),
        storage,
        r.protected
          ? h("td", { class: "act" }, h("span", { class: "lock", text: "Protected", title: r.protected }))
          : h("td", { class: "act" },
              h("button", { class: "btn sm", text: "Quit", onclick: () => void quit([r], false) }),
              h("button", { class: "btn sm", text: "Force quit", onclick: () => void quit([r], true) })));
    }));
    if (!list.length) body.append(h("tr", {}, h("td", { colspan: 6, class: "empty", text: rows.length ? "No app matches" : "Loading\u2026" })));
    paintBulk();
  }

  // the other tabs; built once, loaded each time they are shown
  let tab = store.get<Tab>("ap.tab", "running");
  const panes: Record<Exclude<Tab, "running">, Pane> = { installed: installedPane(), leftovers: leftoversPane(), startup: startupPane() };
  let runningBox!: HTMLElement, tabs!: HTMLElement;
  const TABS: [Tab, string][] = [["running", "Running"], ["installed", "Installed"], ["leftovers", "Leftovers"], ["startup", "Startup"]];

  function showTab(t: Tab) {
    tab = t;
    store.set("ap.tab", t);
    tabs.querySelectorAll("button").forEach((b, i) => b.setAttribute("aria-pressed", String(TABS[i][0] === t)));
    runningBox.hidden = t !== "running";
    bulk.hidden = t !== "running";
    for (const [k, p] of Object.entries(panes)) p.el.hidden = k !== t;
    search.placeholder = t === "startup" ? "Filter startup items   \u2318F" : "Filter apps   \u2318F";
    sub.textContent = t === "running" ? sub.textContent : "";
    if (t === "running") void view.tick!();
    else { const p = panes[t]; p.filter(query); void p.load().catch((e) => toast(String(e), true)); }
  }

  const view: View = {
    interval: 2000,
    mount(root) {
      sub = h("span", { class: "sub" });
      tabs = h("div", { class: "seg", role: "group", "aria-label": "Show" }, ...TABS.map(([t, label]) => h("button", { text: label, onclick: () => showTab(t) })));
      const head = header("Apps", "", tabs);
      head.querySelector(".sub")!.replaceWith(sub);
      search = h("input", { class: "search", type: "search", placeholder: "Filter apps   \u2318F", "aria-label": "Filter apps", value: query });
      search.addEventListener("input", () => { query = search.value; if (tab === "running") paint(); else panes[tab].filter(query); });
      bulk = h("div", { class: "row gap-2" });
      const th = (k: Key, label: string, right = false) => (ths[k] = h("th", { class: right ? "r" : "", text: label, onclick: () => {
        if (key === k) dir = (dir * -1) as 1 | -1; else { key = k; dir = k === "name" ? 1 : -1; }
        paint();
      } }));
      ths = {};
      body = h("tbody");
      runningBox = h("div", { class: "tbl-wrap" }, h("table", { class: "tbl ap" },
        h("colgroup", {}, h("col"), h("col"), h("col"), h("col"), h("col"), h("col")),
        h("thead", {}, h("tr", {}, h("th", {}), th("name", "App"), th("mem", "Memory", true), th("cpu", "CPU", true), th("disk", "Storage", true), h("th", {}))),
        body));
      root.append(head, h("div", { class: "stack" },
        h("div", { class: "toolbar" }, search, bulk),
        runningBox, panes.installed.el, panes.leftovers.el, panes.startup.el));
      paint();
      showTab(tab);
    },
    async tick() {
      if (modal.open) return;
      if (tab !== "running") { await panes[tab].tick?.(); return; }
      rows = await api.appsList();
      paint();
    },
    find() { search.focus(); search.select(); },
    refresh() { void view.tick!(); },
  };
  return view;
}
