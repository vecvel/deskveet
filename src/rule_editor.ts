import { api, Draft, Risk, RuleTest } from "./api";
import { h, openDialog, toast } from "./dom";
import { bytes, env, num, tilde } from "./fmt";

export const CATEGORIES = ["Custom", "Developer", "Package managers", "Browsers", "App caches", "System"];

export const emptyDraft = (): Draft => ({ id: "", name: "", category: "Custom", paths: [], exclude: [], risk: "review", description: "", busy: [] });

const lines = (s: string) => s.split("\n").map((l) => l.trim()).filter(Boolean);
const toHomeTilde = (p: string) => (env.home && p.startsWith(env.home + "/") ? "~" + p.slice(env.home.length) : p);

/** Create or edit a custom rule. Every check that matters runs again in Rust when it is saved. */
export function openRuleEditor(start: Draft, onSaved: (id: string) => void) {
  const d: Draft = { ...start, paths: [...start.paths], exclude: [...start.exclude], busy: [...start.busy] };
  const { dlg, done } = openDialog("wide", "re-title");
  const field = (label: string, el: HTMLElement, help = "") =>
    h("label", { class: "field" }, h("span", { class: "nm", text: label }), el, help ? h("span", { class: "tertiary small", text: help }) : null);

  const name = h("input", { type: "text", value: d.name, placeholder: "Old disk images", "aria-label": "Name" });
  const cat = h("select", { "aria-label": "Category" }, ...CATEGORIES.map((c) => h("option", { value: c, text: c, selected: c === d.category })));
  const risk = h("div", { class: "seg", role: "group", "aria-label": "Risk" });
  const paths = h("textarea", { rows: "4", placeholder: "~/Downloads/*.dmg", "aria-label": "Paths", spellcheck: "false" });
  paths.value = d.paths.join("\n");
  const exclude = h("textarea", { rows: "2", placeholder: "~/Downloads/keep-*.dmg", "aria-label": "Exclude", spellcheck: "false" });
  exclude.value = d.exclude.join("\n");
  const busy = h("input", { type: "text", value: d.busy.join(", "), placeholder: "Xcode, Simulator", "aria-label": "Apps that use it" });
  const desc = h("textarea", { rows: "2", placeholder: "What happens if these are deleted", "aria-label": "Description" });
  desc.value = d.description;
  const preview = h("div", { class: "re-preview" });
  const save = h("button", { class: "btn primary", text: "Save rule", disabled: true });

  const paintRisk = () => {
    risk.replaceChildren(...(["safe", "review", "caution"] as Risk[]).map((r) =>
      h("button", { text: r, "aria-pressed": String(d.risk === r), onclick: () => { d.risk = r; paintRisk(); } })));
  };
  paintRisk();

  const choose = h("button", { class: "btn sm", text: "Choose folder\u2026", onclick: async () => {
    const p = await api.pickFolder();
    if (!p) return;
    paths.value = [...lines(paths.value), `${toHomeTilde(p.replace(/\/$/, ""))}/*`].join("\n");
    schedule();
  } });

  let timer = 0, seq = 0, last: RuleTest | null = null;
  function collect() {
    d.name = name.value; d.category = cat.value; d.paths = lines(paths.value); d.exclude = lines(exclude.value);
    d.busy = busy.value.split(",").map((s) => s.trim()).filter(Boolean); d.description = desc.value;
  }
  function schedule() { clearTimeout(timer); timer = window.setTimeout(() => void test(), 350); }
  async function test() {
    collect();
    const my = ++seq;
    if (!d.paths.length) { last = null; paint(); return; }
    preview.replaceChildren(h("p", { class: "muted", text: "Checking what this matches\u2026" }));
    const t = await api.ruleTest(d).catch((e) => ({ ok: false, error: String(e), count: 0, total: 0, measured: 0, items: [], blocked: [] } as RuleTest));
    if (my === seq) { last = t; paint(); }
  }
  function paint() {
    save.disabled = !last?.ok || !name.value.trim();
    if (!last) { preview.replaceChildren(h("p", { class: "muted", text: "Add a path to see what it matches. Use * for any name, for example ~/Downloads/*.dmg." })); return; }
    const t = last;
    preview.replaceChildren(
      t.error ? h("p", { class: "why bad", text: t.error }) : h("p", {},
        h("b", { text: t.count ? `Matches ${num(t.count)} item${t.count === 1 ? "" : "s"}` : "Matches nothing right now" }),
        t.count ? h("span", { class: "muted", text: ` \u00b7 ${bytes(t.total)}${t.measured < t.count ? ` in the largest ${t.measured}` : ""}. They move to the Trash only after you preview and confirm.` }) : h("span", { class: "muted", text: " The rule can still be saved; it will pick things up as they appear." })),
      ...(t.items.length ? [h("div", { class: "pv-list short" }, ...t.items.slice(0, 12).map((i) =>
        h("div", { class: "pv-row" }, h("div", { class: "pv-main path", title: i.path, text: tilde(i.path) }), h("div", { class: "sz", text: bytes(i.size) }))))] : []),
      ...t.blocked.slice(0, 5).map((b) => h("div", { class: "why bad small", text: `${tilde(b.path)}: ${b.why}` })));
  }

  for (const el of [name, paths, exclude]) el.addEventListener("input", schedule);
  cat.addEventListener("change", () => collect());
  save.addEventListener("click", async () => {
    collect();
    save.disabled = true;
    try { const id = await api.ruleSave(d); toast(start.id ? "Rule saved" : "Rule added"); done(); onSaved(id); }
    catch (e) { toast(String(e), true); save.disabled = false; }
  });

  dlg.append(
    h("h3", { id: "re-title", text: start.id ? "Edit custom rule" : "New custom rule" }),
    h("div", { class: "pv-body re-form" },
      field("Name", name),
      h("div", { class: "row gap-3 wrap" }, field("Category", cat), field("Risk", risk)),
      h("div", { class: "field" }, h("div", { class: "row sb" }, h("span", { class: "nm", text: "Paths" }), choose), paths,
        h("span", { class: "tertiary small", text: "One per line. * matches any name in one folder; {gradle_home} style locations work too. Custom rules always move items to the Trash." })),
      field("Exclude", exclude, "Optional. Items matching these are left alone."),
      field("Apps that use it", busy, "Optional. If one of these is running, cleaning asks you first."),
      field("Description", desc),
      preview),
    h("div", { class: "btns" }, h("button", { class: "btn", text: "Cancel", onclick: done }), save));
  name.focus();
  void test();
}
