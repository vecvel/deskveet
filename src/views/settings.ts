import { api, About, Background, Draft, Location, RuleInfo, RuleResult, RuleState, RuleStatus, Tracking } from "../api";
import { openPreview } from "../preview";
import { emptyDraft, openRuleEditor } from "../rule_editor";
import { openTrack, scheduleText, sparkline, trackSummary } from "../track";
import { confirmDialog } from "../dom";
import { h, toast } from "../dom";
import { bytes, tilde } from "../fmt";
import { store } from "../store";
import { header, View } from "./types";

export type Theme = "system" | "light" | "dark";

export function applyTheme(t: Theme) {
  if (t === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.dataset.theme = t;
}

/** Seconds the window may stay hidden before it is closed to free memory. 0 = never. */
export const releaseSeconds = () => store.get<number>("release", 60);

export const disabledRules = () => new Set(store.get<string[]>("rules.disabled", []));

export function settingsView(): View {
  let about: About | null = null;
  let rules: RuleInfo[] = [];
  let status = new Map<string, RuleStatus>();
  let scanned = new Map<string, RuleResult>();
  let locations: Location[] = [];
  let rulesOpen = store.get("set.rulesOpen", false);
  let locsOpen = store.get("set.locsOpen", false);
  let query = "";
  let filter: "all" | "here" | "missing" | "off" | "custom" = "all";
  let customs: Draft[] = [];
  let tracking: Tracking = { rules: {}, disk_free_pct: 10 };
  let bg: Background | null = null;
  let checking = false;

  async function loadRules(redetect = false) {
    checking = true; render();
    try {
      if (redetect) locations = await api.locationsRedetect();
      const [list, st, cs, locs, cu, tr] = await Promise.all([api.rulesList(), api.rulesStatus(), api.cleanupState(), redetect ? Promise.resolve(locations) : api.locationsInfo(), api.rulesCustom(), api.trackingGet()]);
      rules = list; locations = locs; customs = cu; tracking = tr;
      status = new Map(st.map((x) => [x.id, x]));
      scanned = new Map(cs.results.map((r) => [r.rule.id, r]));
    } catch (e) { toast(String(e), true); }
    checking = false; render();
  }

  const STATE: Record<RuleState, [string, string]> = {
    ready: ["ok", "Ready"], not_installed: ["", "Not on this Mac"], nothing_found: ["", "Nothing found"],
    location_ignored: ["warn", "Location ignored"], needs_fda: ["warn", "Needs Full Disk Access"], on_demand: ["", "In your projects"],
  };
  const applies = (s?: RuleStatus) => !!s && (s.state === "ready" || s.state === "on_demand" || s.state === "needs_fda" || s.state === "location_ignored");
  let box!: HTMLElement;

  function group<T extends string | number>(label: string, key: string, value: T, options: [T, string][], onPick: (v: T) => void) {
    const seg = h("div", { class: "seg", role: "group", "aria-label": label });
    for (const [v, text] of options) {
      seg.append(h("button", { text, "aria-pressed": String(v === value), onclick: () => { store.set(key, v); onPick(v); render(); } }));
    }
    return seg;
  }

  function row(title: string, help: string, control: Node) {
    return h("div", { class: "set-row" }, h("div", { class: "pv-main" }, h("div", { class: "nm", text: title }), h("div", { class: "muted small", text: help })), control);
  }

  function ruleRow(r: RuleInfo, off: Set<string>) {
    const st = status.get(r.id);
    const sc = scanned.get(r.id);
    const sw = h("input", { type: "checkbox", class: "switch", role: "switch", "aria-label": `${r.name} rule` });
    sw.checked = !off.has(r.id);
    sw.addEventListener("change", () => {
      const now = disabledRules();
      sw.checked ? now.delete(r.id) : now.add(r.id);
      store.set("rules.disabled", [...now]);
      render();
    });
    const [cls, label] = st ? STATE[st.state] : ["", "Checking\u2026"];
    const ids = sc?.items.filter((i) => !i.blocked).map((i) => i.id) ?? [];
    const tr = tracking.rules[r.id];
    const right = h("div", { class: "act2" });
    const spark = tr ? sparkline(tr.points) : null;
    if (spark) right.append(spark);
    if (sc && sc.total) right.append(h("div", { class: "sz", text: bytes(sc.total) }));
    const edit = (e: Event, d: Draft) => { e.preventDefault(); openRuleEditor(d, () => void loadRules()); };
    if (r.custom) {
      const draft = customs.find((c) => c.id === r.id);
      right.append(h("div", { class: "row gap-1" },
        ...(draft ? [h("button", { class: "btn sm", text: "Edit", onclick: (e: Event) => edit(e, draft) })] : []),
        h("button", { class: "btn sm", text: "Delete", onclick: async (e: Event) => {
          e.preventDefault();
          if (!(await confirmDialog({ title: `Delete \u201c${r.name}\u201d?`, body: "The rule is removed. Nothing it matched is touched.", ok: "Delete rule", danger: true }))) return;
          await api.ruleDelete(r.id).catch((er) => toast(String(er), true));
          void loadRules();
      void api.backgroundGet().then((b) => { bg = b; render(); }).catch(() => {});
        } })));
    } else if (r.paths.length && r.kind === "path") {
      right.append(h("button", { class: "link", text: "Duplicate", title: "Start a custom rule from this one", onclick: (e: Event) => edit(e, { ...emptyDraft(), name: `${r.name} (custom)`, category: r.category === "Project build artifacts" ? "Custom" : r.category, paths: r.paths, exclude: r.exclude, risk: r.risk, description: r.description }) }));
    }
    if (r.kind !== "artifact") right.append(h("button", { class: "link", text: "Track\u2026", title: "Size alert and automatic cleaning", onclick: (e: Event) => { e.preventDefault(); openTrack(r, tr, () => void loadRules()); } }));
    if (ids.length && sw.checked) right.append(h("button", { class: "btn sm", text: "Clean now", onclick: (e: Event) => { e.preventDefault(); void openPreview({ ids, dryRun: store.get("cl.dry", false), onCleaned: () => void loadRules() }); } }));
    const where = (st?.locations ?? []).map((l) => h("div", { class: "tertiary small", text: l.path ? `Looks in ${tilde(l.path)} (${l.source === "default" ? "default location" : `from ${l.source}`})` : `No ${l.id.replace(/_/g, " ")} folder found` }));
    const skipped = (st?.locations ?? []).flatMap((l) => l.skipped).map((x) => h("div", { class: "why warnx small", text: x }));
    return h("label", { class: "rule-row" + (sw.checked ? "" : " off") }, sw,
      h("div", { class: "pv-main" },
        h("div", { class: "row gap-2 wrap" }, h("span", { class: "nm", text: r.name }), ...(r.custom ? [h("span", { class: "chip", text: "Custom" })] : []),
          h("span", { class: `pill ${r.risk === "safe" ? "ok" : r.risk === "review" ? "warn" : "bad"}`, text: r.risk }),
          h("span", { class: "status" }, h("b", { class: cls, text: label }), st?.note && st.state !== "ready" ? ` ${st.note}` : st?.matches ? ` ${st.matches} found` : "")),
        h("div", { class: "muted small", text: r.description }), ...where, ...skipped,
        ...(tr && (trackSummary(tr) || tr.alert || tr.schedule) ? [h("div", { class: "tertiary small", text: [trackSummary(tr), tr.alert ? `alert at ${bytes(tr.alert)}` : "", tr.schedule ? `cleans ${scheduleText(tr.schedule).toLowerCase()}` : ""].filter(Boolean).join(" \u00b7 ") })] : [])),
      right);
  }

  function rulesCard() {
    const off = disabledRules();
    const here = rules.filter((r) => applies(status.get(r.id))).length;
    const fda = rules.filter((r) => status.get(r.id)?.state === "needs_fda").length;
    const card = h("section", { class: "card" },
      h("button", { class: "disclose", "aria-expanded": String(rulesOpen), onclick: () => { rulesOpen = !rulesOpen; store.set("set.rulesOpen", rulesOpen); render(); } },
        h("span", { class: "chev", text: rulesOpen ? "\u25be" : "\u25b8" }),
        h("span", { class: "pv-main" }, h("div", { class: "nm", text: "Cleanup rules" }),
          h("div", { class: "muted small", text: rules.length
            ? `${rules.length - off.size} of ${rules.length} on \u00b7 ${here} apply on this Mac${fda ? ` \u00b7 ${fda} need Full Disk Access` : ""}. Rules that are off are skipped by scans and can't be cleaned.`
            : "\u2026" }))));
    if (!rulesOpen) return card;
    const search = h("input", { class: "search", type: "search", placeholder: "Filter rules", "aria-label": "Filter rules", value: query });
    search.addEventListener("input", () => { query = search.value; render(); card.ownerDocument.querySelector<HTMLInputElement>(".rules-tools .search")?.focus(); });
    const seg = h("div", { class: "seg", role: "group", "aria-label": "Show" });
    for (const [k, t] of [["all", "All"], ["here", "On this Mac"], ["missing", "Not here"], ["off", "Off"], ["custom", "Custom"]] as const) {
      seg.append(h("button", { text: t, "aria-pressed": String(filter === k), onclick: () => { filter = k; render(); } }));
    }
    card.append(h("div", { class: "toolbar rules-tools mt-3" }, search, seg,
      h("button", { class: "btn", text: checking ? "Checking\u2026" : "Check again", disabled: checking, onclick: () => void loadRules() })),
      h("div", { class: "row gap-2 mt-2" },
        h("button", { class: "btn primary", text: "Add rule", onclick: () => openRuleEditor(emptyDraft(), () => { filter = "custom"; void loadRules(); }) }),
        h("button", { class: "btn", text: "Import\u2026", onclick: async () => {
          const r = await api.rulesImport().catch((e) => { toast(String(e), true); return null; });
          if (!r) return;
          toast(`Imported ${r.added} rule${r.added === 1 ? "" : "s"}${r.skipped.length ? `, skipped ${r.skipped.length}: ${r.skipped[0]}` : ""}`, r.skipped.length > 0);
          void loadRules();
      void api.backgroundGet().then((b) => { bg = b; render(); }).catch(() => {});
        } }),
        h("button", { class: "btn", text: "Export\u2026", disabled: !customs.length, onclick: async () => {
          const p = await api.rulesExport().catch((e) => { toast(String(e), true); return null; });
          if (p) toast(`Exported to ${tilde(p)}`);
        } })));
    const q = query.trim().toLowerCase();
    const shown = rules.filter((r) => {
      const st = status.get(r.id);
      if (q && !(r.name.toLowerCase().includes(q) || r.category.toLowerCase().includes(q) || r.id.includes(q))) return false;
      if (filter === "here") return applies(st);
      if (filter === "missing") return !!st && !applies(st);
      if (filter === "off") return off.has(r.id);
      if (filter === "custom") return r.custom;
      return true;
    });
    for (const cat of [...new Set(shown.map((r) => r.category))]) {
      card.append(h("div", { class: "rules-cat" }, h("h4", { text: cat }), ...shown.filter((x) => x.category === cat).map((r) => ruleRow(r, off))));
    }
    if (!shown.length) card.append(h("p", { class: "muted mt-3", text: "No rule matches." }));
    return card;
  }

  function locationsCard() {
    const card = h("section", { class: "card" },
      h("button", { class: "disclose", "aria-expanded": String(locsOpen), onclick: () => { locsOpen = !locsOpen; store.set("set.locsOpen", locsOpen); render(); } },
        h("span", { class: "chev", text: locsOpen ? "\u25be" : "\u25b8" }),
        h("span", { class: "pv-main" }, h("div", { class: "nm", text: "Tool locations" }),
          h("div", { class: "muted small", text: "Where Deskveet found each tool's folders: asked the tool, read your shell's variables, or used the default." }))));
    if (!locsOpen) return card;
    card.append(h("div", { class: "toolbar mt-3" }, h("span", { class: "sp" }),
      h("button", { class: "btn", text: checking ? "Looking\u2026" : "Re-detect", disabled: checking, onclick: () => void loadRules(true) })));
    for (const l of locations) {
      card.append(h("div", { class: "rule-row" },
        h("div", { class: "pv-main" },
          h("div", { class: "nm", text: l.id.replace(/_/g, " ") }),
          h("div", { class: "path", text: l.path ? tilde(l.path) : "Not found on this Mac" }),
          ...l.skipped.map((x) => h("div", { class: "why warnx small", text: x }))),
        h("span", { class: "tertiary small", text: l.path ? (l.source === "default" ? "default" : l.source) : "" })));
    }
    return card;
  }

  function bgSwitch(label: string, on: boolean, set: (on: boolean) => Promise<Background>) {
    const sw = h("input", { type: "checkbox", class: "switch", role: "switch", "aria-label": label });
    sw.checked = on;
    sw.addEventListener("change", async () => {
      try { bg = await set(sw.checked); } catch (e) { toast(String(e), true); }
      render();
    });
    return sw;
  }

  function render() {
    const theme = store.get<Theme>("theme", "system");
    const rel = releaseSeconds();
    box.replaceChildren(
      h("section", { class: "card" }, h("h2", { text: "Appearance" }),
        row("Theme", "Follows macOS by default.", group<Theme>("Theme", "theme", theme, [["system", "System"], ["light", "Light"], ["dark", "Dark"]], applyTheme))),
      h("section", { class: "card" }, h("h2", { text: "Memory" }),
        row("Close the window when hidden", "The window is what costs memory (about 70 MB in WebKit helper processes). After this long out of sight it is closed; Deskveet keeps running and reopens instantly from the Dock.",
          group<number>("Release delay", "release", rel, [[30, "30 s"], [60, "1 min"], [300, "5 min"], [0, "Never"]], () => {}))),
      rulesCard(),
      locationsCard(),
      h("section", { class: "card" }, h("h2", { text: "Background" }),
        row("Keep Deskveet in the menu bar", "Free space, memory and the next scheduled clean in the menu bar, with \u201cClean safe caches now\u201d. Closing the window also hides the Dock icon. Alerts and schedules keep working either way while Deskveet runs.",
          bgSwitch("Menu bar", !!bg?.menubar, (on) => api.backgroundSet({ menubar: on }))),
        row("Open at login", bg?.login_needs_approval ? "Waiting for your approval in System Settings \u2192 General \u2192 Login Items." : "Starts Deskveet in the background when you log in, so schedules and alerts run without opening it.",
          bgSwitch("Open at login", !!bg?.login, (on) => api.backgroundSet({ login: on }))),
        ...(bg ? [h("p", { class: "tertiary small mt-2", text: `Watching the folders of ${bg.watched_rules} rules for changes: sizes update within seconds while this window is open and at most every 10 minutes in the background.` })] : [])),
      h("section", { class: "card" }, h("h2", { text: "Alerts" }),
        row("Disk almost full", "A notification when your start-up disk has less free space than this. At most one a day.",
          group<number>("Disk alert", "disk-alert-ui", Math.round(tracking.disk_free_pct ?? 0), [[0, "Off"], [5, "5%"], [10, "10%"], [15, "15%"], [20, "20%"]], (v) => { tracking.disk_free_pct = v || null; void api.diskAlertSet(v || null); })),
        row("Test", "Shows a notification now, so you can allow Deskveet in System Settings if macOS asks.",
          h("button", { class: "btn", text: "Send a test notification", onclick: () => void api.notifyTest() }))),
      h("section", { class: "card" }, h("h2", { text: "Data on this Mac" }),
        row("Scan cache", about ? `${bytes(about.cache_bytes)} in ${tilde(about.cache_dir)}. Safe to delete; the next scan is just slower.` : "…",
          h("button", { class: "btn", text: "Clear scan cache", onclick: async () => { const n = await api.clearScanCache(); toast(`Removed ${n} cache file${n === 1 ? "" : "s"}`); about = await api.about(); render(); } })),
        row("Undo log", about ? tilde(about.undo_log) : "…",
          h("button", { class: "btn", text: "Show in Finder", onclick: () => about && void api.reveal(about.undo_log).catch((e) => toast(String(e), true)) }))),
      h("section", { class: "card" }, h("h2", { text: "Privacy" }),
        h("p", { class: "facts" }, "Deskveet works entirely offline. It has no network code, no telemetry and no accounts, and the app\u2019s content security policy forbids every network connection. Cleanup tools you choose to run (for example brew or npm) are separate programs and behave as they always do."),
        about ? h("p", { class: "muted small", text: `Version ${about.version}` }) : null),
    );
  }

  return {
    mount(root) {
      box = h("div", { class: "cleanup" });
      root.append(header("Settings"), box);
      render();
      void api.about().then((a) => { about = a; render(); }).catch(() => {});
      void loadRules();
      void api.backgroundGet().then((b) => { bg = b; render(); }).catch(() => {});
    },
  };
}
