import { api, Children, Row, ScanDone, ScanMeta, ScanProgress } from "../api";
import { h, toast } from "../dom";
import { ago, bytes, bytesParts, date, env, num, secs, tilde } from "../fmt";
import { icons } from "../icons";
import { store } from "../store";
import { squarify } from "../treemap";
import { fda, FDA_HELP } from "../fda";
import { openPreview } from "../preview";
import { header, View } from "./types";

type SortKey = "name" | "size" | "mtime";
const MAP_MAX = 70; // cells in the treemap; the rest is folded into one "other" cell

export function storageView(): View {
  // ── state (lives as long as the app; the DOM is rebuilt on every mount) ──
  let root = "";
  let meta: ScanMeta | null = null;
  let cwd = store.get("st.cwd", "");
  let data: Children | null = null;
  let mode = store.get<"list" | "map">("st.mode", "list");
  let sortKey = store.get<SortKey>("st.sort", "size");
  let sortDir = store.get<1 | -1>("st.dir", -1);
  let filter = "";
  let progress: ScanProgress | null = null;
  let booted = false;
  let staleNote = false;
  let mounted = false;
  let unlisten: (() => void)[] = [];
  let unFda: (() => void) | null = null;

  // ── dom refs (valid while mounted) ──
  let box!: HTMLElement, sub!: HTMLElement, search!: HTMLInputElement;
  let ro: ResizeObserver | null = null;

  const scanning = () => progress !== null;
  /** The scanned folder as people say it: "Home" for the home folder, otherwise its path with ~. */
  const place = (p: string) => (p === env.home ? "Home" : tilde(p));

  async function boot() {
    const st = await api.scanStatus();
    env.home = st.home;
    root = root || st.home;
    if (st.running) { progress = st.running; root = st.running.root; }
    if (st.loaded) { meta = st.loaded; root = st.loaded.root; }
    else if (!st.running) { meta = await api.scanLoadCached(root); if (meta) root = meta.root; }
    if (meta) { cwd = cwd && cwd.startsWith(root) ? cwd : root; await load(cwd); }
    booted = true;
  }

  async function load(path: string) {
    try { data = await api.scanChildren(path); cwd = path; store.set("st.cwd", path); }
    catch (e) { toast(String(e), true); if (path !== root) await load(root); }
  }

  async function start(full: boolean) {
    if (scanning()) return;
    try {
      progress = { root, files: 0, bytes: 0, dirs: 0, reused: 0, current: "" };
      render();
      await api.scanStart(root, full);
    } catch (e) { progress = null; toast(String(e), true); render(); }
  }

  async function onDone(d: ScanDone) {
    progress = null;
    if (d.error && !d.meta) toast(d.error, true);
    else if (d.error) toast(d.error, true);
    if (d.meta) { meta = d.meta; root = d.meta.root; staleNote = false; await load(root); }
    if (mounted) render();
  }

  // ── actions ──
  const act = (fn: (p: string) => Promise<void>, path: string) => fn(path).catch((e) => toast(String(e), true));
  async function copy(path: string) {
    try { await navigator.clipboard.writeText(path); toast("Path copied"); }
    catch {
      const t = h("textarea", { class: "offscreen" }); t.value = path; document.body.append(t); t.select();
      document.execCommand("copy"); t.remove(); toast("Path copied");
    }
  }
  async function choose() {
    const p = await api.pickFolder();
    if (!p) return;
    root = p; meta = null; data = null; cwd = "";
    const cached = await api.scanLoadCached(p);
    if (cached) { meta = cached; root = cached.root; await load(root); render(); }
    else { render(); void start(false); }
  }

  // ── rendering ──
  function actions(r: Row) {
    if (r.kind === "small_dirs" || r.kind === "small_files") return h("td", { class: "act" });
    const b = (icon: string, label: string, fn: () => void) => {
      const el = h("button", { class: "ibtn", title: label, "aria-label": label, onclick: (e: Event) => { e.stopPropagation(); fn(); } });
      el.innerHTML = icon;
      return el;
    };
    return h("td", { class: "act" },
      b(icons.reveal, "Reveal in Finder", () => act(api.reveal, r.path)),
      b(icons.open, "Open", () => act(api.open, r.path)),
      b(icons.copy, "Copy path", () => void copy(r.path)),
      b(icons.look, "Quick Look", () => act(api.quickLook, r.path)),
      b(icons.trash, "Move to Trash…", () => void openPreview({ picks: [r.path], dryRun: false, onCleaned: () => { staleNote = true; void load(cwd).then(render); toast("Moved to the Trash. Rescan to update the sizes."); } })));
  }

  function visibleRows(): Row[] {
    if (!data) return [];
    const q = filter.trim().toLowerCase();
    const rows = data.rows.filter((r) => !q || r.name.toLowerCase().includes(q) || r.path.toLowerCase().includes(q));
    const cmp = {
      name: (a: Row, b: Row) => a.name.localeCompare(b.name),
      size: (a: Row, b: Row) => a.size - b.size,
      mtime: (a: Row, b: Row) => a.mtime - b.mtime,
    }[sortKey];
    return rows.sort((a, b) => sortDir * cmp(a, b));
  }

  function listView(rows: Row[]) {
    const total = data?.size || 1;
    const th = (key: SortKey, label: string, cls = "") => h("th", {
      class: cls, text: label, "aria-sort": sortKey === key ? (sortDir === 1 ? "ascending" : "descending") : null,
      onclick: () => { if (sortKey === key) sortDir = (sortDir * -1) as 1 | -1; else { sortKey = key; sortDir = key === "name" ? 1 : -1; } store.set("st.sort", sortKey); store.set("st.dir", sortDir); render(); },
    });
    const body = h("tbody");
    for (const r of rows) {
      const drill = r.kind === "dir";
      const name = drill
        ? h("button", { class: "link nm", text: r.name, title: `Open ${r.name}`, onclick: () => void go(r.path) })
        : h("span", { class: "nm" + (r.kind === "file" ? "" : " muted"), text: r.name });
      const [v, u] = bytesParts(r.size);
      const pct = (r.size / total) * 100;
      body.append(h("tr", { class: drill ? "drill" : "", ondblclick: () => drill && void go(r.path) },
        h("td", {},
          h("div", { class: "row gap-2" }, ico(r.kind === "dir" ? icons.folder : r.kind === "file" ? icons.file : icons.stack), name,
            r.kind === "dir" ? h("span", { class: "pid", text: `${num(r.files)} files` }) : null),
          r.kind === "dir" || r.kind === "file" ? h("div", { class: "path", title: r.path, text: tilde(r.path) }) : h("div", { class: "path", text: "Too small to list individually" })),
        h("td", { class: "r" }, h("div", { class: "sz", text: `${v} ${u}` }), h("div", { class: "mbar" }, h("i", { style: `width:${Math.max(0.5, pct).toFixed(1)}%` }))),
        h("td", { class: "r muted", text: date(r.mtime) }),
        actions(r)));
    }
    if (!rows.length) body.append(h("tr", {}, h("td", { colspan: 4, class: "empty", text: filter ? "Nothing matches the filter" : "This folder has nothing above 1 MB" })));
    return h("div", { class: "tbl-wrap" }, h("table", { class: "tbl st" },
      h("colgroup", {}, h("col"), h("col"), h("col"), h("col")),
      h("thead", {}, h("tr", {}, th("name", "Name"), th("size", "Size", "r"), th("mtime", "Modified", "r"), h("th", {}))), body));
  }

  function mapView(rows: Row[]) {
    const wrap = h("div", { class: "tmap", role: "list", "aria-label": "Treemap of this folder" });
    const sorted = [...rows].filter((r) => r.size > 0).sort((a, b) => b.size - a.size);
    const shown = sorted.slice(0, MAP_MAX);
    const rest = sorted.slice(MAP_MAX).reduce((s, r) => s + r.size, 0);
    const cells: { r: Row | null; size: number }[] = shown.map((r) => ({ r, size: r.size }));
    if (rest) cells.push({ r: null, size: rest });
    const draw = () => {
      const W = wrap.clientWidth, H = wrap.clientHeight;
      if (!W || !H) return;
      const rects = squarify(cells.map((c) => c.size), W, H);
      wrap.replaceChildren(...cells.map((c, i) => {
        const q = rects[i], r = c.r;
        const kind = !r ? "other" : r.kind === "dir" ? "dir" : r.kind === "file" ? "file" : "other";
        const el = h("div", {
          class: `cell ${kind}`, role: "listitem", tabindex: r?.kind === "dir" ? "0" : null,
          style: `left:${q.x}px;top:${q.y}px;width:${Math.max(0, q.w - 2)}px;height:${Math.max(0, q.h - 2)}px;--c:${kind === "file" ? "var(--slate)" : ["var(--accent)", "var(--sand)", "var(--ok)"][i % 3]};--a:${(Math.max(16, 40 - i * 1.4) / 100).toFixed(2)}`,
          title: r ? `${r.name}\n${tilde(r.path)}\n${bytes(r.size)}` : `${sorted.length - MAP_MAX} more items · ${bytes(rest)}`,
          onclick: () => r?.kind === "dir" && void go(r.path),
          onkeydown: (e: KeyboardEvent) => { if (e.key === "Enter" && r?.kind === "dir") void go(r.path); },
        });
        if (q.w > 74 && q.h > 36) el.append(h("b", { text: r ? r.name : "Other" }), h("span", { text: bytes(c.size) }));
        return el;
      }));
    };
    ro?.disconnect();
    ro = new ResizeObserver(() => draw());
    ro.observe(wrap);
    queueMicrotask(draw);
    return wrap;
  }

  const ico = (svg: string) => { const s = document.createElement("span"); s.className = "ico"; s.innerHTML = svg; return s; };

  async function go(path: string) { await load(path); filter = ""; render(); }

  function crumbs() {
    const rel = cwd.startsWith(root) ? cwd.slice(root.length).split("/").filter(Boolean) : [];
    const items: [string, string][] = [[root === env.home ? "Home" : root.split("/").pop() || root, root]];
    let p = root;
    for (const part of rel) { p = `${p}/${part}`; items.push([part, p]); }
    const nav = h("nav", { class: "crumbs", "aria-label": "Folder path", title: cwd });
    nav.append(ico(icons.folder));
    items.forEach(([label, path], i) => {
      if (i) nav.append(h("span", { class: "sep", text: "›" }));
      nav.append(i === items.length - 1 ? h("span", { class: "here", text: label }) : h("button", { class: "link", text: label, onclick: () => void go(path) }));
    });
    return nav;
  }

  function progressCard() {
    const p = progress!;
    const [bv, bu] = bytesParts(p.bytes);
    return h("section", { class: "card scan" },
      h("div", { class: "row sb" },
        h("div", {}, h("h2", { text: "Scanning" }), h("div", { class: "big" }, num(p.files), h("small", { text: `files · ${bv} ${bu}` }))),
        h("button", { class: "btn", text: "Cancel", onclick: () => void api.scanCancel() })),
      h("div", { class: "bar" }, h("i", { class: "c-used indet" })),
      h("div", { class: "muted ell", title: p.current, text: p.current ? tilde(p.current) : "Starting…" }),
      p.reused ? h("div", { class: "muted", text: `Re-using ${num(p.reused)} unchanged folders from the last scan` }) : null);
  }

  function empty() {
    return h("section", { class: "card cta" },
      h("h2", { text: "Find out what is using your disk" }),
      h("p", { class: "muted", text: `Deskveet will read file sizes in ${root === env.home ? "your home folder" : tilde(root)}. Nothing leaves your Mac and nothing is changed.` }),
      h("div", { class: "row center mt-3" },
        h("button", { class: "btn primary", text: "Scan now", onclick: () => void start(false) }),
        h("button", { class: "btn", text: "Choose another folder…", onclick: () => void choose() })));
  }

  function fdaBanner() {
    if (!meta || !meta.protected_n) return null;
    if (fda.get() === "granted") {
      return h("div", { class: "banner info" },
        h("span", { text: `Full Disk Access is on. Rescan to include the ${num(meta.protected_n)} folders macOS blocked last time.` }),
        h("button", { class: "btn sm", text: "Rescan", onclick: () => void start(false) }));
    }
    return h("div", { class: "banner fda" },
      h("span", { text: `macOS blocked ${num(meta.protected_n)} folders, so their size is missing from these totals. ${FDA_HELP}` }),
      h("button", { class: "btn sm", text: "Open Full Disk Access settings", onclick: () => void api.openFda().catch((e) => toast(String(e), true)) }));
  }

  function render() {
    if (!mounted) return;
    ro?.disconnect();
    const f = document.activeElement === search;
    box.replaceChildren();
    sub.textContent = meta ? `${staleNote ? "Changed since the scan · " : ""}${place(meta.root)} · ${num(meta.files)} files · ${bytes(meta.bytes)} · scanned ${ago(meta.scanned_at)} in ${secs(meta.duration_ms)}` : place(root);
    if (scanning()) box.append(progressCard());
    if (!meta || !data) { if (!scanning() && booted) box.append(empty()); return; }
    const rows = visibleRows();
    const fda = fdaBanner();
    if (fda) box.append(fda);
    box.append(
      crumbs(),
      h("div", { class: "toolbar" }, search,
        h("div", { class: "seg", role: "group", "aria-label": "View" },
          h("button", { text: "List", "aria-pressed": String(mode === "list"), onclick: () => { mode = "list"; store.set("st.mode", mode); render(); } }),
          h("button", { text: "Treemap", "aria-pressed": String(mode === "map"), onclick: () => { mode = "map"; store.set("st.mode", mode); render(); } }))),
      mode === "list" ? listView(rows) : mapView(rows));
    if (f) search.focus();
  }

  const view: View = {
    mount(rootEl) {
      mounted = true;
      search = h("input", { class: "search", type: "search", placeholder: "Filter this folder   ⌘F", "aria-label": "Filter this folder", value: filter });
      search.addEventListener("input", () => { filter = search.value; render(); });
      sub = h("span", { class: "sub" });
      const head = header("Storage");
      head.querySelector(".sub")!.replaceWith(sub);
      head.append(
        h("button", { class: "btn", text: "Choose folder…", onclick: () => void choose() }),
        h("button", { class: "btn primary", text: "Rescan", title: "⌘R, hold ⇧ for a full re-scan", onclick: (e: MouseEvent) => void start(e.shiftKey) }));
      box = h("div", { class: "stack" });
      rootEl.append(head, box);
      void Promise.all([
        api.listen<ScanProgress>("scan-progress", (p) => { progress = p; if (mounted) updateProgress(); }),
        api.listen<ScanDone>("scan-done", (d) => void onDone(d)),
      ]).then((u) => { unlisten.forEach((f) => f()); unlisten = u; });
      unFda = fda.subscribe(() => render());
      void fda.refresh();
      if (booted) render();
      else void boot().then(render).catch((e) => { booted = true; toast(String(e), true); render(); });
    },
    unmount() { mounted = false; ro?.disconnect(); unFda?.(); },
    find() { search?.focus(); search?.select(); },
    refresh(full) { void start(!!full); },
  };

  // Update only the numbers while scanning: no DOM rebuild per event.
  function updateProgress() {
    const card = box?.querySelector(".scan");
    if (!card || !progress) { render(); return; }
    const [bv, bu] = bytesParts(progress.bytes);
    card.querySelector(".big")!.innerHTML = `${num(progress.files)}<small>files · ${bv} ${bu}</small>`;
    const cur = card.querySelector<HTMLElement>(".ell")!;
    cur.textContent = progress.current ? tilde(progress.current) : "Starting…";
    cur.title = progress.current;
  }

  return view;
}
