import { api, CItem, RuleResult, Risk } from "../api";
import { h, toast } from "../dom";
import { ago, bytes, date, num, tilde } from "../fmt";
import { fda, FDA_HELP } from "../fda";
import { openPreview } from "../preview";
import { store } from "../store";
import { openUndoLog } from "../undolog";
import { artifactsView } from "./artifacts";
import { disabledRules } from "./settings";
import { header, View } from "./types";

const ORDER = ["Custom", "Developer", "Package managers", "Browsers", "App caches", "System"];
const riskCls = (r: Risk) => (r === "safe" ? "ok" : r === "review" ? "warn" : "bad");

export function cleanupView(): View {
  let results: RuleResult[] = [];
  let scanning = false;
  let version = "";
  let scannedAt = 0;
  let dryRun = store.get("cl.dry", false);
  const selected = new Set<string>();
  const expanded = new Set<string>(store.get<string[]>("cl.open", []));
  let booted = false;
  let mounted = false;
  let unlisten: (() => void)[] = [];
  let raf = 0;
  let box!: HTMLElement, sub!: HTMLElement, bar!: HTMLElement, abox!: HTMLElement;
  let unFda: (() => void) | null = null;
  let tab = store.get<"caches" | "artifacts">("cl.tab", "caches");
  const art = artifactsView();
  let artMounted = false;

  function showTab() {
    box.hidden = tab !== "caches";
    abox.hidden = tab !== "artifacts";
    bar.hidden = tab !== "caches" || selected.size === 0;
    if (tab === "artifacts" && !artMounted) { art.mount(abox); artMounted = true; }
    if (tab === "caches" && artMounted) { art.unmount?.(); abox.replaceChildren(); artMounted = false; }
    if (tab === "caches" && booted && !results.length && !scanning) void rescan();
    segBtns.forEach(([t, b]) => b.setAttribute("aria-pressed", String(t === tab)));
    if (tab === "artifacts") sub.textContent = "Build folders in your projects";
    else render();
  }
  const segBtns: [string, HTMLElement][] = [];

  const allItems = () => results.flatMap((r) => r.items);
  const selectable = (it: CItem) => !it.blocked;
  const size = (ids: Iterable<string>) => { const s = new Set(ids); return allItems().filter((i) => s.has(i.id)).reduce((a, i) => a + i.size, 0); };

  async function boot() {
    const st = await api.cleanupState();
    version = st.rules_version;
    results = st.results;
    scanning = st.running;
    if (results.length) scannedAt = Date.now() / 1000;
    booted = true;
    if (!results.length && !scanning && tab === "caches") await rescan();
  }

  async function rescan() {
    if (scanning) return;
    selected.clear();
    results = [];
    scanning = true;
    render();
    try { await api.cleanupScan(); } catch (e) { scanning = false; toast(String(e), true); render(); }
  }

  function upsert(rr: RuleResult) {
    const i = results.findIndex((r) => r.rule.id === rr.rule.id);
    if (i >= 0) results[i] = rr; else results.push(rr);
    schedule();
  }

  function schedule() {
    if (raf || !mounted) return;
    raf = requestAnimationFrame(() => { raf = 0; render(); });
  }

  function ruleState(r: RuleResult) {
    const sel = r.items.filter((i) => selected.has(i.id)).length;
    const can = r.items.filter(selectable).length;
    return { sel, can, all: can > 0 && sel === can, some: sel > 0 && sel < can };
  }

  function toggleRule(r: RuleResult, on: boolean) {
    for (const i of r.items) if (selectable(i)) on ? selected.add(i.id) : selected.delete(i.id);
    render();
  }

  function selectSafe() {
    selected.clear();
    const off = disabledRules();
    for (const r of results) if (r.rule.risk === "safe" && !r.rule.opt_in && !off.has(r.rule.id)) for (const i of r.items) if (selectable(i)) selected.add(i.id);
    render();
  }

  function ruleRow(r: RuleResult) {
    const st = ruleState(r);
    const open = expanded.has(r.rule.id);
    const cb = h("input", { type: "checkbox", "aria-label": `Select all in ${r.rule.name}`, disabled: st.can === 0 });
    cb.checked = st.all; cb.indeterminate = st.some;
    cb.addEventListener("change", () => toggleRule(r, cb.checked));
    const how = r.rule.method === "trash" ? "Moves to Trash" : r.rule.method === "erase" ? "Erases permanently" : `Runs ${r.rule.tool ?? "a tool"}`;
    const head = h("div", { class: "rule-head" },
      cb,
      h("button", { class: "rule-toggle", "aria-expanded": String(open), onclick: () => { open ? expanded.delete(r.rule.id) : expanded.add(r.rule.id); store.set("cl.open", [...expanded]); render(); } },
        h("span", { class: "chev", text: open ? "▾" : "▸" }),
        h("span", { class: "nm", text: r.rule.name }),
        h("span", { class: `pill ${riskCls(r.rule.risk)}`, text: r.rule.risk }),
        r.rule.opt_in ? h("span", { class: "pill", text: "opt-in" }) : null,
        r.running.length ? h("span", { class: "pill warn", title: `${r.running.join(", ")} is running`, text: "in use" }) : null),
      h("span", { class: "muted cnt", text: `${r.items.length} item${r.items.length === 1 ? "" : "s"}` }),
      h("span", { class: "sz", text: bytes(r.total) }));
    const el = h("div", { class: "rule" + (open ? " open" : "") }, head);
    if (open) {
      el.append(h("div", { class: "rule-body" },
        h("div", { class: "facts" },
          h("div", {}, h("b", { text: "If you delete it: " }), r.rule.description),
          h("div", {}, h("b", { text: "Comes back: " }), r.rule.regenerates),
          h("div", {}, h("b", { text: "Method: " }), how)),
        h("div", { class: "items" }, ...r.items.slice(0, 200).map((it) => itemRow(it)),
          r.items.length > 200 ? h("div", { class: "muted", text: `…and ${r.items.length - 200} smaller items (select the whole group to include them)` }) : null)));
    }
    return el;
  }

  function itemRow(it: CItem) {
    const cb = h("input", { type: "checkbox", "aria-label": `Select ${it.label}`, disabled: !selectable(it) });
    cb.checked = selected.has(it.id);
    cb.addEventListener("change", () => { cb.checked ? selected.add(it.id) : selected.delete(it.id); render(); });
    return h("label", { class: "item" + (it.blocked ? " off" : "") },
      cb,
      h("div", { class: "pv-main" },
        h("div", { class: "nm", text: it.label }),
        it.path ? h("div", { class: "path", title: it.path, text: tilde(it.path) }) : null,
        it.info.length || it.mtime ? h("div", { class: "muted small", text: [...it.info, it.mtime ? `last changed ${date(it.mtime)}` : ""].filter(Boolean).join(" · ") }) : null,
        it.blocked ? h("div", { class: "why bad", text: it.blocked }) : null),
      h("span", { class: "sz", text: bytes(it.size) }));
  }

  function renderBar() {
    bar.hidden = selected.size === 0;
    if (bar.hidden) return;
    bar.replaceChildren(
      h("div", {}, h("b", { text: `${num(selected.size)} item${selected.size === 1 ? "" : "s"}` }), h("span", { class: "muted", text: ` · ${bytes(size(selected))} selected` })),
      h("div", { class: "row" },
        h("button", { class: "btn", text: "Clear", onclick: () => { selected.clear(); render(); } }),
        h("button", { class: "btn primary", text: dryRun ? "Preview dry run…" : "Preview & clean…", onclick: () => void openPreview({ ids: [...selected], dryRun, onCleaned: () => { selected.clear(); void rescan(); } }) })));
  }

  function render() {
    if (!mounted) return;
    const off = disabledRules();
    const live = results.filter((r) => !off.has(r.rule.id));
    const reclaim = live.reduce((s, r) => s + r.total, 0);
    const safe = live.filter((r) => r.rule.risk === "safe" && !r.rule.opt_in).reduce((s, r) => s + r.total, 0);
    sub.textContent = scanning ? `Scanning… ${results.length} rule${results.length === 1 ? "" : "s"} checked` : results.length ? `${bytes(reclaim)} found · rules ${version}${scannedAt ? ` · scanned ${ago(scannedAt)}` : ""}` : "";
    box.replaceChildren();

    box.append(h("section", { class: "card sum" },
      h("div", { class: "row sb" },
        h("div", {}, h("div", { class: "big" }, bytes(safe), h("small", { text: "safe to clean" })),
          h("div", { class: "muted", text: scanning ? "Looking at caches, build folders and tools…" : "Nothing is selected or deleted until you preview and confirm." })),
        h("div", { class: "col" },
          scanning ? h("button", { class: "btn", text: "Stop", onclick: () => void api.cleanupCancel() }) : h("button", { class: "btn", text: "Select all safe", onclick: selectSafe, disabled: safe === 0 }),
          h("label", { class: "ack sm", title: "Preview and log what would happen without changing anything" },
            Object.assign(h("input", { type: "checkbox", checked: dryRun }), { onchange: (e: Event) => { dryRun = (e.target as HTMLInputElement).checked; store.set("cl.dry", dryRun); render(); } }),
            h("span", { text: "Dry run" })))),
      scanning ? h("div", { class: "bar" }, h("i", { class: "c-used indet" })) : null));

    const needsFda = results.find((r) => r.note?.includes("Full Disk Access"));
    if (needsFda && fda.get() === "granted") {
      box.append(h("div", { class: "banner info" },
        h("span", { text: "Full Disk Access is on. Rescan to see what is in the Trash." }),
        h("button", { class: "btn sm", text: "Rescan", onclick: () => void rescan() })));
    } else if (needsFda) {
      box.append(h("div", { class: "banner fda" },
        h("span", { text: `Deskveet can't see the Trash yet. ${FDA_HELP}` }),
        h("button", { class: "btn sm", text: "Open Full Disk Access settings", onclick: () => void api.openFda().catch((e) => toast(String(e), true)) })));
    }
    const withItems = live.filter((r) => r.available && r.items.length);
    for (const cat of ORDER) {
      const rs = withItems.filter((r) => r.rule.category === cat).sort((a, b) => b.total - a.total);
      if (!rs.length) continue;
      box.append(h("section", { class: "card cat" },
        h("h2", {}, cat, h("span", { class: "sp" }), h("span", { class: "muted", text: bytes(rs.reduce((s, r) => s + r.total, 0)) })),
        ...rs.map(ruleRow)));
    }
    const nothing = results.filter((r) => r.available && !r.items.length).length;
    const missing = results.filter((r) => !r.available && !r.note?.includes("Full Disk Access"));
    if (!scanning && results.length) {
      box.append(h("p", { class: "muted foot-note" },
        `${nothing} rule${nothing === 1 ? "" : "s"} found nothing to clean.`,
        missing.length ? ` Not installed here: ${missing.map((m) => m.rule.name).join(", ")}.` : ""));
    }
    renderBar();
    bar.hidden = tab !== "caches" || selected.size === 0;
  }

  const view: View = {
    mount(root) {
      mounted = true;
      sub = h("span", { class: "sub" });
      const head = header("Cleanup");
      head.querySelector(".sub")!.replaceWith(sub);
      const seg = h("div", { class: "seg", role: "group", "aria-label": "What to clean" });
      segBtns.length = 0;
      for (const [t, label] of [["caches", "Caches & tools"], ["artifacts", "Project artifacts"]] as const) {
        const b = h("button", { text: label, onclick: () => { tab = t; store.set("cl.tab", t); showTab(); } });
        segBtns.push([t, b]); seg.append(b);
      }
      head.append(seg,
        h("button", { class: "btn", text: "Undo log", onclick: () => void openUndoLog() }),
        h("button", { class: "btn primary", text: "Rescan", title: "⌘R", onclick: () => view.refresh!() }));
      box = h("div", { class: "cleanup" });
      abox = h("div", { hidden: true });
      bar = h("div", { class: "actionbar", hidden: true });
      artMounted = false;
      root.append(head, box, abox, bar);
      void Promise.all([
        api.listen<RuleResult>("cleanup-rule", (rr) => upsert(rr)),
        api.listen<{ cancelled: boolean }>("cleanup-done", async () => { scanning = false; scannedAt = Date.now() / 1000; const st = await api.cleanupState(); if (st.results.length) results = st.results; render(); }),
      ]).then((u) => { unlisten.forEach((f) => f()); unlisten = u; });
      unFda = fda.subscribe(() => { if (tab === "caches") render(); });
      void fda.refresh();
      if (booted) { render(); showTab(); }
      else void boot().then(() => { render(); showTab(); }).catch((e) => { booted = true; toast(String(e), true); render(); showTab(); });
    },
    unmount() { mounted = false; unFda?.(); cancelAnimationFrame(raf); raf = 0; if (artMounted) { art.unmount?.(); artMounted = false; } },
    find() { if (tab === "artifacts") art.find?.(); },
    refresh() { if (tab === "artifacts") art.refresh?.(); else void rescan(); },
  };
  return view;
}
