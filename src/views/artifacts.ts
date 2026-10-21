import { api, ArtifactScan, Project } from "../api";
import { h, toast } from "../dom";
import { bytes, num, tilde } from "../fmt";
import { openPreview } from "../preview";
import { store } from "../store";
import { View } from "./types";

const DAY = 86400;

export function artifactsView(): View {
  let roots = store.get<string[]>("ar.roots", []);
  let days = store.get<number>("ar.days", 60);
  let scan: ArtifactScan | null = null;
  let scanning = false;
  /// another cleanup job (the caches scan, a clean) holds the single worker slot
  let busy = false;
  let dirs = 0;
  let defaults: string[] = [];
  const selected = new Set<string>();
  const open = new Set<string>();
  let query = "";
  let mounted = false;
  let booted = false;
  let unlisten: (() => void)[] = [];
  let box!: HTMLElement, bar!: HTMLElement;

  const idle = (p: Project) => Math.max(0, Math.floor((Date.now() / 1000 - p.last_active) / DAY));
  const isStale = (p: Project) => idle(p) >= days;
  const selectable = (p: Project) => p.items.filter((i) => !i.blocked);
  const sizeOf = () => (scan?.projects ?? []).flatMap((p) => p.items).filter((i) => selected.has(i.id)).reduce((s, i) => s + i.size, 0);

  async function boot() {
    const st = await api.artifactState();
    defaults = st.default_roots;
    // Desktop/Documents/Downloads make macOS show a privacy prompt the first time: offer them, never pre-select.
    if (!roots.length) roots = defaults.filter((d) => !/\/(Desktop|Documents|Downloads)$/.test(d)).slice(0, 4);
    scan = st.scan;
    busy = st.busy;
    booted = true;
  }

  async function start() {
    if (scanning) return;
    if (busy) { toast("Another cleanup job is still running. Try again in a moment.", true); return; }
    if (!roots.length) { toast("Add at least one folder to look in.", true); return; }
    store.set("ar.roots", roots);
    selected.clear(); scanning = true; dirs = 0;
    render();
    try { await api.artifactScan(roots); } catch (e) { scanning = false; toast(String(e), true); render(); }
  }

  async function addFolder() {
    const p = await api.pickFolder();
    if (p && !roots.includes(p)) { roots.push(p); store.set("ar.roots", roots); render(); }
  }

  function selectStale() {
    selected.clear();
    for (const p of scan?.projects ?? []) if (isStale(p)) for (const i of selectable(p)) selected.add(i.id);
    render();
  }

  function projectRow(p: Project) {
    const ids = selectable(p).map((i) => i.id);
    const n = ids.filter((i) => selected.has(i)).length;
    const cb = h("input", { type: "checkbox", "aria-label": `Select all build folders in ${p.name}`, disabled: ids.length === 0 });
    cb.checked = ids.length > 0 && n === ids.length; cb.indeterminate = n > 0 && n < ids.length;
    cb.addEventListener("change", () => { for (const i of ids) cb.checked ? selected.add(i) : selected.delete(i); render(); });
    const isOpen = open.has(p.path);
    const stale = isStale(p);
    const row = h("div", { class: "rule" + (isOpen ? " open" : "") },
      h("div", { class: "rule-head" }, cb,
        h("button", { class: "rule-toggle", "aria-expanded": String(isOpen), onclick: () => { isOpen ? open.delete(p.path) : open.add(p.path); render(); } },
          h("span", { class: "chev", text: isOpen ? "▾" : "▸" }),
          h("span", { class: "pv-main" },
            h("span", { class: "nm", text: p.name }),
            h("span", { class: "path block", title: p.path, text: tilde(p.path) }))),
        h("span", { class: "chips" }, ...p.items.map((i) => h("span", { class: "chip", title: `${i.rule_name}: ${tilde(i.path)}`, text: `${i.dir} ${bytes(i.size)}` }))),
        h("span", { class: `pill ${stale ? "warn" : ""}`, title: "Time since any file in the project changed (build folders excluded)", text: p.last_active ? `${idle(p)} d idle` : "unknown" }),
        h("span", { class: "sz", text: bytes(p.total) })));
    if (isOpen) {
      row.append(h("div", { class: "rule-body" }, h("div", { class: "items" }, ...p.items.map((i) => {
        const c = h("input", { type: "checkbox", "aria-label": `Select ${i.dir}`, disabled: !!i.blocked });
        c.checked = selected.has(i.id);
        c.addEventListener("change", () => { c.checked ? selected.add(i.id) : selected.delete(i.id); render(); });
        return h("label", { class: "item" + (i.blocked ? " off" : "") }, c,
          h("div", { class: "pv-main" }, h("div", { class: "nm", text: `${i.dir}  ·  ${i.rule_name}` }), h("div", { class: "path", title: i.path, text: tilde(i.path) }),
            i.blocked ? h("div", { class: "why bad", text: i.blocked }) : null),
          h("span", { class: "sz", text: bytes(i.size) }));
      }))));
    }
    return row;
  }

  function render() {
    if (!mounted) return;
    box.replaceChildren();
    const staleTotal = (scan?.projects ?? []).filter(isStale).reduce((s, p) => s + p.total, 0);

    const daysIn = h("input", { type: "number", min: "0", max: "3650", value: String(days), class: "num", "aria-label": "Days without changes" });
    daysIn.addEventListener("change", () => { days = Math.max(0, Math.min(3650, Number(daysIn.value) || 0)); store.set("ar.days", days); render(); });
    box.append(h("section", { class: "card" },
      h("h2", { text: "Look for build folders in" }),
      h("div", { class: "chips roots" }, ...roots.map((r) => h("span", { class: "chip root", title: r }, tilde(r),
        h("button", { class: "x", "aria-label": `Remove ${r}`, text: "×", onclick: () => { roots = roots.filter((x) => x !== r); store.set("ar.roots", roots); render(); } }))),
        h("button", { class: "btn sm", text: "Add folder…", onclick: () => void addFolder() }),
        ...defaults.filter((d) => !roots.includes(d)).slice(0, 4).map((d) => h("button", { class: "link", title: /\/(Desktop|Documents)$/.test(d) ? "macOS asks for permission the first time" : "", text: `+ ${tilde(d)}`, onclick: () => { roots.push(d); store.set("ar.roots", roots); render(); } }))),
      h("div", { class: "row wrap mt-3" },
        h("button", { class: "btn primary", text: scanning ? "Scanning…" : busy ? "Waiting for the other scan…" : scan ? "Rescan" : "Find build folders", disabled: scanning || busy, onclick: () => void start() }),
        scanning ? h("button", { class: "btn", text: "Stop", onclick: () => void api.cleanupCancel() }) : null,
        h("span", { class: "muted", text: scanning ? `${num(dirs)} folders looked at…` : "Only folders next to a project file (package.json, Cargo.toml, pubspec.yaml, …) are listed." }),
        h("span", { class: "sp" }))));

    if (scanning && !scan) { renderBar(); return; }
    if (!scan) { renderBar(); return; }

    const q = query.trim().toLowerCase();
    const list = scan.projects.filter((p) => !q || p.path.toLowerCase().includes(q));
    box.append(h("section", { class: "card sum" },
      h("div", { class: "row sb wrap gap-3" },
        h("div", {}, h("div", { class: "big" }, bytes(scan.total), h("small", { text: `in ${num(scan.projects.length)} projects` })),
          h("div", { class: "muted", text: `${bytes(staleTotal)} belongs to projects untouched for ${days}+ days` })),
        h("div", { class: "row" },
          h("label", { class: "muted row gap-1" }, "Stale after", daysIn, "days"),
          h("button", { class: "btn", text: "Select stale", onclick: selectStale, disabled: staleTotal === 0 }),
          h("button", { class: "btn", text: "Select none", onclick: () => { selected.clear(); render(); }, disabled: selected.size === 0 })))));
    const search = h("input", { class: "search", type: "search", placeholder: "Filter projects   ⌘F", value: query, "aria-label": "Filter projects" });
    search.addEventListener("input", () => { query = search.value; render(); box.querySelector<HTMLInputElement>(".search")?.focus(); });
    box.append(h("div", { class: "toolbar" }, search));
    box.append(list.length
      ? h("section", { class: "card cat" }, ...list.slice(0, 300).map(projectRow))
      : h("div", { class: "empty", text: scan.projects.length ? "No project matches" : "No build folders found in these folders." }));
    if (list.length > 300) box.append(h("p", { class: "muted foot-note", text: `Showing the largest 300 of ${list.length} projects.` }));
    renderBar();
  }

  function renderBar() {
    bar.hidden = selected.size === 0;
    if (bar.hidden) return;
    const dry = store.get("cl.dry", false);
    bar.replaceChildren(
      h("div", {}, h("b", { text: `${num(selected.size)} build folder${selected.size === 1 ? "" : "s"}` }), h("span", { class: "muted", text: ` · ${bytes(sizeOf())} selected` })),
      h("div", { class: "row" },
        h("button", { class: "btn", text: "Clear", onclick: () => { selected.clear(); render(); } }),
        h("button", { class: "btn primary", text: dry ? "Preview dry run…" : "Preview & clean…", onclick: () => void openPreview({ ids: [...selected], dryRun: dry, onCleaned: () => { selected.clear(); scan = null; void start(); } }) })));
  }

  const view: View = {
    mount(root) {
      mounted = true;
      box = h("div", { class: "cleanup" });
      bar = h("div", { class: "actionbar", hidden: true });
      root.append(box, bar);
      void Promise.all([
        api.listen<{ dirs: number }>("artifact-progress", (p) => { dirs = p.dirs; if (mounted && scanning) { const m = box.querySelector(".muted"); if (m && scanning) m.textContent = `${num(dirs)} folders looked at…`; } }),
        api.listen<ArtifactScan>("artifact-done", (s) => { scanning = false; busy = false; scan = s; render(); }),
        api.listen<unknown>("cleanup-done", () => { busy = false; render(); }),
      ]).then((u) => { unlisten.forEach((f) => f()); unlisten = u; });
      if (booted) render(); else void boot().then(render).catch((e) => { booted = true; toast(String(e), true); render(); });
    },
    unmount() { mounted = false; bar?.remove(); },
    find() { box?.querySelector<HTMLInputElement>(".search")?.focus(); },
    refresh() { void start(); },
  };
  return view;
}
