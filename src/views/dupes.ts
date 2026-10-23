// Storage → Duplicates: identical files below the current folder. Finding only reads files; removing goes
// through the normal preview and the Trash, and the last copy of a file is never offered.
import { api, DupDone, DupGroup, DupProgress, DupResult } from "../api";
import { h, toast } from "../dom";
import { ago, bytes, date, num, tilde } from "../fmt";
import { icons } from "../icons";
import { openPreview } from "../preview";
import { store } from "../store";

const LIST_STEP = 100;

const ibtn = (icon: string, label: string, fn: () => void) => {
  const el = h("button", { class: "ibtn", title: label, "aria-label": label, onclick: fn });
  el.innerHTML = icon;
  return el;
};
const nameOf = (p: string) => p.slice(p.lastIndexOf("/") + 1);
/** What removing a copy frees: its own blocks when the system knows them (an APFS clone has none). */
const frees = (g: DupGroup, path: string) => { const c = g.copies.find((x) => x.path === path); return c?.own ?? g.size; };

export interface DupesPane {
  /** The pane for `cwd`, with the view's filter text applied. The same element every time. */
  view(cwd: string, filter: string): HTMLElement;
}

export function dupesPane(place: (p: string) => string, changed: () => void): DupesPane {
  let res: DupResult | null = null;
  let prog: DupProgress | null = null;
  let minMb = store.get<number>("st.dMin", 10);
  let cwd = "", filter = "";
  let limit = LIST_STEP;
  const ticked = new Set<string>();
  const box = h("div", { class: "stack" });

  void api.dupesState().then((s) => { res = s.result; prog = s.running; paint(); });
  void api.listen<DupProgress>("dupes-progress", (p) => { const first = !prog; prog = p; first ? paint() : updateProgress(); });
  void api.listen<DupDone>("dupes-done", async (d) => {
    prog = null;
    if (d.error) toast(d.error, true);
    if (!d.cancelled && !d.error) { res = (await api.dupesState()).result; ticked.clear(); limit = LIST_STEP; }
    paint();
  });

  async function start() {
    try { prog = { root: cwd, files: 0, read: 0, to_read: 0, comparing: false, current: "" }; paint(); await api.dupesStart(cwd, minMb); }
    catch (e) { prog = null; toast(String(e), true); paint(); }
  }

  function progressText(p: DupProgress) {
    return p.comparing
      ? `Comparing contents · ${bytes(p.read)} of ${bytes(p.to_read)}`
      : `Listing files · ${num(p.files)} of ${minMb >= 1000 ? "1 GB" : `${minMb} MB`} or more`;
  }

  function updateProgress() {
    const el = box.querySelector(".dup-prog");
    if (!el || !prog) { paint(); return; }
    el.querySelector(".big-line")!.textContent = progressText(prog);
    const bar = el.querySelector<HTMLElement>(".bar > i")!;
    bar.classList.toggle("indet", !prog.comparing || !prog.to_read);
    bar.style.width = prog.comparing && prog.to_read ? `${Math.min(100, (prog.read / prog.to_read) * 100)}%` : "";
    const cur = el.querySelector<HTMLElement>(".ell")!;
    cur.textContent = prog.comparing ? "" : tilde(prog.current);
  }

  function selectExtra() {
    if (!res) return;
    ticked.clear();
    for (const g of res.groups) g.copies.slice(1).forEach((c) => { if (!c.locked) ticked.add(c.path); });
    paint();
  }

  async function review() {
    const picks = [...ticked];
    try { await api.dupesCheck(picks); } catch (e) { toast(String(e), true); return; }
    void openPreview({
      picks, dryRun: false,
      onCleaned: async () => {
        res = await api.dupesPrune();
        for (const p of [...ticked]) if (!res?.groups.some((g) => g.copies.some((c) => c.path === p))) ticked.delete(p);
        toast("Moved to the Trash. Rescan to update the folder sizes.");
        changed();
        paint();
      },
    });
  }

  function groupCard(g: DupGroup) {
    const own = g.copies.map((c) => c.own);
    const shared = own.every((o) => o != null) && own.reduce((s, o) => s + o!, 0) < g.size * 0.1 * g.copies.length;
    const rows = g.copies.map((c, i) => {
      const cb = h("input", { type: "checkbox", "aria-label": `Move ${tilde(c.path)} to the Trash`, disabled: !!c.locked });
      cb.checked = ticked.has(c.path);
      cb.addEventListener("change", () => {
        if (cb.checked && g.copies.every((x) => x.path === c.path || ticked.has(x.path))) {
          cb.checked = false;
          toast("Keep at least one copy.");
          return;
        }
        cb.checked ? ticked.add(c.path) : ticked.delete(c.path);
        paintTotals();
      });
      return h("label", { class: "rule-row dup-row" }, cb,
        h("div", { class: "pv-main" },
          h("div", { class: "path", title: c.path, text: tilde(c.path) }),
          h("div", { class: "tertiary small", text: `Modified ${date(c.mtime)}${i === 0 ? " · oldest" : ""}` })),
        c.locked ? h("span", { class: "lock", title: c.locked, text: "Protected" }) : null,
        h("div", { class: "row gap-2" },
          ibtn(icons.reveal, "Reveal in Finder", () => void api.reveal(c.path).catch((e) => toast(String(e), true))),
          ibtn(icons.look, "Quick Look", () => void api.quickLook(c.path).catch((e) => toast(String(e), true)))));
    });
    return h("div", { class: "card flush" },
      h("div", { class: "rule-row" },
        h("div", { class: "pv-main" },
          h("div", { class: "nm", text: nameOf(g.copies[0].path) }),
          h("div", { class: "tertiary small", text: shared
            ? `${g.copies.length} copies of ${bytes(g.size)} · they already share space on disk, so removing one frees little`
            : `${g.copies.length} copies of ${bytes(g.size)}` })),
        h("span", { class: "sz", title: "Space taken by the extra copies", text: bytes(g.size * (g.copies.length - 1)) })),
      ...rows);
  }

  const totals = h("span", { class: "muted" });
  const go = h("button", { class: "btn primary", text: "Review and move to Trash…", onclick: () => void review() });
  function paintTotals() {
    if (!res) return;
    let sum = 0;
    for (const g of res.groups) for (const c of g.copies) if (ticked.has(c.path)) sum += frees(g, c.path);
    totals.textContent = ticked.size ? `${num(ticked.size)} selected · frees about ${bytes(sum)}` : "Nothing selected";
    go.toggleAttribute("disabled", ticked.size === 0);
  }

  function paint() {
    if (!cwd) return;
    const seg = h("div", { class: "row gap-2" }, h("span", { class: "muted", text: "At least" }),
      h("div", { class: "seg", role: "group", "aria-label": "Minimum size" }, ...([[1, "1 MB"], [10, "10 MB"], [100, "100 MB"]] as const).map(([v, t]) =>
        h("button", { text: t, "aria-pressed": String(v === minMb), disabled: !!prog, onclick: () => { minMb = v; store.set("st.dMin", v); paint(); } }))));
    const here = res && res.root === cwd && res.min_mb === minMb;
    const action = prog
      ? h("button", { class: "btn", text: "Cancel", onclick: () => void api.dupesCancel() })
      : h("button", { class: "btn primary", text: here ? "Search again" : `Find duplicates in ${place(cwd)}`, onclick: () => void start() });
    const parts: (HTMLElement | null)[] = [
      h("div", { class: "toolbar" }, seg, h("span", { class: "sp" }), action),
      h("p", { class: "muted small", text: "Compares the contents of files, not their names. Your Library, apps, photo and music libraries, .git and node_modules are left out, and files that are only in iCloud are not downloaded." }),
    ];
    if (prog) {
      parts.push(h("section", { class: "card dup-prog" },
        h("div", { class: "big-line", text: progressText(prog) }),
        h("div", { class: "bar" }, h("i", { class: "c-used indet" })),
        h("div", { class: "muted ell", text: tilde(prog.current) })));
    } else if (!res) {
      parts.push(h("p", { class: "empty", text: `Nothing searched yet. Searching ${place(cwd)} reads every file of ${minMb} MB or more, which can take a few minutes for a large folder.` }));
    }
    if (res) {
      const q = filter.trim().toLowerCase();
      const list = res.groups.filter((g) => !q || g.copies.some((c) => c.path.toLowerCase().includes(q)));
      const extra = res.groups.reduce((s, g) => s + g.size * (g.copies.length - 1), 0) + res.more_bytes;
      const count = res.groups.length + res.more;
      parts.push(
        h("div", { class: "muted small", text: `${place(res.root)} · ${res.min_mb} MB or more · searched ${ago(res.found_at)}` }),
        h("div", { class: "toolbar" },
          h("b", { text: count ? `${num(count)} file${count === 1 ? "" : "s"} with copies · ${bytes(extra)} in extra copies` : "No duplicates found" }),
          h("span", { class: "sp" }),
          count ? h("button", { class: "btn", text: "Select extra copies", title: "Keeps the oldest copy of each file", onclick: selectExtra }) : null,
          ticked.size ? h("button", { class: "btn", text: "Clear", onclick: () => { ticked.clear(); paint(); } }) : null),
        res.more ? h("p", { class: "muted small", text: `Showing the ${num(res.groups.length)} that take the most space. Search with a larger minimum size to see the rest.` }) : null,
        ...list.slice(0, limit).map(groupCard),
        list.length > limit ? h("button", { class: "btn", text: `Show ${Math.min(LIST_STEP, list.length - limit)} more`, onclick: () => { limit += LIST_STEP; paint(); } }) : null,
        !list.length && count ? h("p", { class: "empty", text: "Nothing matches" }) : null,
        count ? h("div", { class: "btns sticky-foot" }, totals, h("span", { class: "sp" }), go) : null);
      paintTotals();
    }
    box.replaceChildren(...parts.filter((p): p is HTMLElement => !!p));
  }

  return {
    view(dir, q) {
      if (dir !== cwd) limit = LIST_STEP;
      cwd = dir; filter = q;
      paint();
      return box;
    },
  };
}
