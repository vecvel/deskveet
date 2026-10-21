import { api, MethodName, Plan, PlanItem, Report, Risk } from "./api";
import { h, openDialog, toast } from "./dom";
import { bytes, tilde } from "./fmt";

const riskPill = (r: Risk) => h("span", { class: `pill ${r === "safe" ? "ok" : r === "review" ? "warn" : "bad"}`, text: r });

function methodText(i: { method: MethodName; command: string | null; recoverable: boolean }): string {
  if (i.method === "trash") return "Moves to the Trash";
  if (i.method === "erase") return "Erases permanently";
  return `Runs: ${i.command ?? "tool"}${i.recoverable ? "" : " · not recoverable"}`;
}

export interface PreviewOpts {
  ids?: string[];
  picks?: string[];
  dryRun: boolean;
  /** Called after a real (non dry-run) clean finished. */
  onCleaned?: (r: Report) => void;
}

/** Preview -> explicit confirmation -> run -> report. Nothing happens before the confirm button. */
export async function openPreview(o: PreviewOpts): Promise<void> {
  let plan: Plan;
  try { plan = await api.cleanupPlan({ ids: o.ids ?? [], picks: o.picks ?? [], dry_run: o.dryRun }); }
  catch (e) { toast(String(e), true); return; }

  const { dlg, done } = openDialog("wide", "pv-title");
  const body = h("div", { class: "pv-body" });
  const foot = h("div", { class: "btns" });
  dlg.append(h("h3", { id: "pv-title", text: plan.dry_run ? "Dry run: nothing will be changed" : "Review before cleaning" }), body, foot);

  const acked = new Set<string>();
  let eraseOk = false;
  const runnable = () => plan.items.filter((i) => !i.blocked);
  const willRun = (i: PlanItem) => !i.blocked && (i.confirm.length === 0 || acked.has(i.id)) && (i.method !== "erase" || eraseOk);

  function bar() {
    const n = plan.items.filter(willRun).length;
    const sum = plan.items.filter(willRun).reduce((s, i) => s + i.size, 0);
    go.disabled = n === 0;
    go.textContent = plan.dry_run ? `Run dry run (${n})` : `${plan.items.some((i) => i.method !== "trash" && willRun(i)) ? "Clean" : "Move to Trash"} · ${bytes(sum)}`;
  }

  const go = h("button", { class: "btn primary", text: "…", onclick: () => void run() });
  const cancel = h("button", { class: "btn", text: "Cancel", onclick: done });
  foot.append(cancel, go);

  function draw() {
    const total = runnable().reduce((s, i) => s + i.size, 0);
    const blocked = plan.items.length - runnable().length;
    body.replaceChildren(
      h("p", { class: "muted" },
        `${plan.items.length} item${plan.items.length === 1 ? "" : "s"} · ${bytes(total)}`,
        blocked ? ` · ${blocked} cannot be cleaned` : "",
        plan.dry_run ? " · the undo log will record this dry run" : " · every action is written to the undo log"),
      h("div", { class: "pv-list" }, ...plan.items.map((i) => {
        const row = h("div", { class: "pv-row" + (i.blocked ? " off" : "") });
        row.append(
          h("div", { class: "pv-main" },
            h("div", { class: "row", style: "gap:8px" }, h("b", { text: i.label }), riskPill(i.risk), h("span", { class: "muted", text: i.rule_name })),
            i.path ? h("div", { class: "path", title: i.path, text: tilde(i.path) }) : null,
            h("div", { class: "how" + (i.method === "trash" ? "" : " warnx"), text: methodText(i) }),
            i.blocked ? h("div", { class: "why bad", text: `Skipped: ${i.blocked}` }) : null,
            i.admin_cmd ? h("code", { class: "cmd", text: i.admin_cmd }) : null,
            ...i.confirm.map((c) => h("label", { class: "ack" },
              Object.assign(h("input", { type: "checkbox" }), { onchange: (e: Event) => { (e.target as HTMLInputElement).checked ? acked.add(i.id) : acked.delete(i.id); bar(); } }),
              h("span", { text: c })))),
          h("div", { class: "sz", text: i.blocked ? "—" : bytes(i.size) }));
        return row;
      })),
    );
    if (plan.erase && !plan.dry_run) {
      body.append(h("label", { class: "ack erase" },
        Object.assign(h("input", { type: "checkbox" }), { onchange: (e: Event) => { eraseOk = (e.target as HTMLInputElement).checked; bar(); } }),
        h("span", { text: "I understand that items marked “Erases permanently” are deleted for good and cannot be restored." })));
    }
    bar();
  }

  async function run() {
    go.disabled = true; cancel.disabled = true;
    const prog = h("div", { class: "bar thick" }, h("i", { class: "c-used", style: "width:0%" }));
    const cur = h("div", { class: "muted ell", text: "Starting…" });
    body.replaceChildren(h("p", { text: plan.dry_run ? "Running dry run…" : "Cleaning…" }), prog, cur);
    const un = await api.listen<{ done: number; total: number; current: string }>("cleanup-progress", (p) => {
      (prog.firstElementChild as HTMLElement).style.width = `${p.total ? (p.done / p.total) * 100 : 0}%`;
      cur.textContent = p.current ? `${p.done + 1} of ${p.total}: ${p.current}` : "Finishing…";
    });
    try {
      const rep = await api.cleanupRun({ token: plan.token, confirm_erase: eraseOk, overrides: [...acked] });
      un();
      showReport(rep);
      if (!rep.dry_run) o.onCleaned?.(rep);
    } catch (e) {
      un();
      body.replaceChildren(h("p", { class: "why bad", text: String(e) }));
      cancel.disabled = false; cancel.textContent = "Close";
    }
  }

  function showReport(r: Report) {
    dlg.querySelector("h3")!.textContent = r.dry_run ? "Dry run finished" : r.failed ? "Finished with problems" : "Done";
    const lines: Node[] = [];
    if (r.dry_run) lines.push(h("p", {}, `Would have processed ${r.ok} item${r.ok === 1 ? "" : "s"}. Nothing was changed.`));
    else {
      lines.push(h("div", { class: "big" }, bytes(r.trashed_bytes + r.freed_bytes), h("small", { text: "cleaned" })));
      if (r.trashed_bytes) lines.push(h("p", {}, `${bytes(r.trashed_bytes)} moved to the Trash. That space comes back when you empty the Trash.`));
      if (r.freed_bytes) lines.push(h("p", {}, `${bytes(r.freed_bytes)} freed immediately.`));
      if (r.trashed_bytes) lines.push(h("p", { class: "muted", text: "To restore: open the Trash and drag items back, or use Undo log → Restore." }));
    }
    if (r.skipped) lines.push(h("p", { class: "muted", text: `${r.skipped} skipped.` }));
    body.replaceChildren(
      ...lines,
      h("div", { class: "pv-list short" }, ...r.results.map((x) => h("div", { class: "pv-row" },
        h("div", { class: "pv-main" },
          h("div", { class: "row", style: "gap:8px" }, h("b", { text: x.label }),
            h("span", { class: `pill ${x.status === "ok" || x.status === "dry-run" ? "ok" : x.status === "failed" ? "bad" : "warn"}`, text: x.status })),
          x.error ? h("div", { class: "why " + (x.status === "failed" ? "bad" : ""), text: x.error }) : null),
        h("div", { class: "sz", text: x.status === "ok" || x.status === "dry-run" ? bytes(x.bytes) : "—" })))));
    foot.replaceChildren();
    if (r.trashed_bytes) foot.append(h("button", { class: "btn", text: "Open Trash", onclick: () => void api.openTrash() }));
    foot.append(h("button", { class: "btn primary", text: "Close", onclick: done }));
    (foot.lastElementChild as HTMLElement).focus();
  }

  draw();
  cancel.focus(); // the safe choice is the default
}
