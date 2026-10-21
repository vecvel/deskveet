import { api, ProcRow } from "../api";
import { confirmDialog, h, modal, toast } from "../dom";
import { pct, ram, tilde } from "../fmt";
import { header, View } from "./types";

type Col = "name" | "pid" | "user" | "mem" | "cpu";

export function processesView(): View {
  let sortKey: Col = "mem";
  let sortDir: 1 | -1 = -1;
  let query = "";
  let rows: ProcRow[] = [];
  let total = 0;
  let body!: HTMLElement, search!: HTMLInputElement, count!: HTMLElement;
  let ths: Record<string, HTMLElement> = {};
  let busy = false;

  const cmp: Record<Col, (a: ProcRow, b: ProcRow) => number> = {
    name: (a, b) => a.name.localeCompare(b.name),
    pid: (a, b) => a.pid - b.pid,
    user: (a, b) => a.user.localeCompare(b.user),
    mem: (a, b) => a.mem - b.mem,
    cpu: (a, b) => a.cpu - b.cpu,
  };

  function paintHead() {
    for (const [k, th] of Object.entries(ths)) {
      if (k === sortKey) th.setAttribute("aria-sort", sortDir === 1 ? "ascending" : "descending");
      else th.removeAttribute("aria-sort");
    }
  }

  function paint() {
    const sorted = [...rows].sort((a, b) => sortDir * cmp[sortKey](a, b));
    count.textContent = `${sorted.length} of ${total} processes`;
    if (!sorted.length) { body.replaceChildren(h("tr", {}, h("td", { colspan: 5, class: "empty", text: "No matching processes" }))); return; }
    body.replaceChildren(...sorted.map(row));
  }

  function row(r: ProcRow) {
    const locked = r.protected;
    const why = locked ? `Protected: ${r.protected}` : "";
    const actions = locked
      ? h("td", { class: "act" }, h("span", { class: "lock", text: "Protected", title: why }))
      : h("td", { class: "act" },
          h("button", { class: "btn sm", text: "Quit", title: `Ask ${r.name} to quit`, onclick: () => act(r, false) }),
          h("button", { class: "btn sm", text: "Force Quit", title: `Kill ${r.name} immediately`, onclick: () => act(r, true) }));
    const where = r.path
      ? h("div", { class: "path", title: r.path },
          h("button", { class: "link", text: "Open in Finder", onclick: () => api.reveal(r.reveal ?? r.path!).catch((e) => toast(String(e), true)) }),
          ` · ${tilde(r.path)}`)
      : h("div", { class: "path", text: "path unavailable" });
    return h("tr", {},
      h("td", {}, h("div", { class: "nm", title: r.name }, r.name, h("span", { class: "pid", text: String(r.pid) })), where),
      h("td", { text: r.user }),
      h("td", { class: "r", text: ram(r.mem) }),
      h("td", { class: "r", text: pct(r.cpu, 1) }),
      actions);
  }

  async function act(r: ProcRow, force: boolean) {
    const ok = await confirmDialog({
      title: `${force ? "Force Quit" : "Quit"} “${r.name}”?`,
      body: force
        ? `PID ${r.pid} will be killed immediately. Unsaved work in it will be lost.`
        : `PID ${r.pid} will be asked to quit. The app may ask to save its work first.`,
      ok: force ? "Force Quit" : "Quit",
      danger: force,
    });
    if (!ok) return;
    try { await api.quit(r.pid, r.start, force); toast(`${r.name} ${force ? "was force quit" : "was asked to quit"}`); }
    catch (e) { toast(String(e), true); }
    void view.tick!();
  }

  const view: View = {
    interval: 2000,
    mount(root) {
      search = h("input", { class: "search", type: "search", placeholder: "Filter by name, PID or path   ⌘F", "aria-label": "Filter processes" });
      search.addEventListener("input", () => { query = search.value; void view.tick!(); });
      count = h("span", { class: "sub" });
      const th = (key: Col, label: string, right = false) => (ths[key] = h("th", { class: right ? "r" : "", text: label, onclick: () => {
        if (sortKey === key) sortDir = (sortDir * -1) as 1 | -1; else { sortKey = key; sortDir = key === "name" || key === "user" || key === "pid" ? 1 : -1; }
        paintHead(); paint(); if (key === "mem" || key === "cpu") void view.tick!();
      } }));
      body = h("tbody");
      const seg = h("div", { class: "seg", role: "group", "aria-label": "Sort" });
      const head = header("Processes", "", count);
      root.append(head,
        h("div", { class: "toolbar" }, search, seg),
        h("div", { class: "tbl-wrap" },
          h("table", { class: "tbl pr" },
            h("colgroup", {}, h("col"), h("col"), h("col"), h("col"), h("col")),
            h("thead", {}, h("tr", {}, th("name", "Name  ·  PID"), th("user", "User"), th("mem", "RAM", true), th("cpu", "CPU", true), h("th", {}))),
            body)));
      const mk = (label: string, key: "mem" | "cpu") => h("button", { text: label, "aria-pressed": String(sortKey === key), onclick: () => {
        sortKey = key; sortDir = -1; seg.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String(b === btn))); paintHead(); void view.tick!();
      } });
      const btn = mk("Top RAM", "mem");
      seg.append(btn, mk("Top CPU", "cpu"));
      paintHead();
    },
    async tick() {
      if (modal.open || busy) return; // never reshuffle rows under an open confirmation
      busy = true;
      try {
        const l = await api.processes({ sort: sortKey === "cpu" ? "cpu" : "mem", limit: 120, query });
        rows = l.rows; total = l.total; paint();
      } finally { busy = false; }
    },
    find() { search.focus(); search.select(); },
    refresh() { void view.tick!(); },
  };
  return view;
}
