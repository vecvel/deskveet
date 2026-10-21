import { api, Point, RuleInfo, Schedule, TrackInfo } from "./api";
import { h, openDialog, toast } from "./dom";
import { ago, bytes } from "./fmt";

const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const hh = (n: number) => `${String(n).padStart(2, "0")}:00`;

export function scheduleText(s: Schedule): string {
  return s.every === "daily" ? `Daily at ${hh(s.hour)}` : `${DAYS[s.weekday].slice(0, 3)} at ${hh(s.hour)}`;
}

/** Tiny size-history line (inline SVG, colours from tokens). */
export function sparkline(points: Point[]): SVGElement | null {
  if (points.length < 2) return null;
  const W = 96, H = 22, ts = points.map((p) => p.t), bs = points.map((p) => p.b);
  const t0 = Math.min(...ts), t1 = Math.max(...ts), b0 = Math.min(...bs), b1 = Math.max(...bs);
  const x = (t: number) => ((t - t0) / Math.max(1, t1 - t0)) * W;
  const y = (b: number) => H - 2 - ((b - b0) / Math.max(1, b1 - b0)) * (H - 4);
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  svg.setAttribute("class", "spark-line");
  svg.setAttribute("aria-hidden", "true");
  const pl = document.createElementNS(ns, "polyline");
  pl.setAttribute("points", points.map((p) => `${x(p.t).toFixed(1)},${y(p.b).toFixed(1)}`).join(" "));
  svg.append(pl);
  return svg;
}

/** "growing 1.2 GB a week · cleaned 3 d ago · 4.1 GB freed so far" */
export function trackSummary(t?: TrackInfo): string {
  if (!t) return "";
  const bits: string[] = [];
  if (t.growth_per_day != null && Math.abs(t.growth_per_day) * 7 >= 1e6) {
    const w = t.growth_per_day * 7;
    bits.push(w > 0 ? `growing ${bytes(w)} a week` : `shrinking ${bytes(-w)} a week`);
  }
  if (t.last_cleaned) bits.push(`cleaned ${ago(t.last_cleaned)}`);
  if (t.freed) bits.push(`${bytes(t.freed)} freed so far`);
  return bits.join(" \u00b7 ");
}

/** Alert and schedule settings for one rule. */
export function openTrack(rule: RuleInfo, info: TrackInfo | undefined, onSaved: () => void) {
  const { dlg, done } = openDialog("", "tr-title");
  const gb = h("input", { type: "number", min: "0", step: "0.5", class: "num", "aria-label": "Alert size in GB", value: info?.alert ? String(+(info.alert / 1e9).toFixed(1)) : "" });
  const every = h("select", { "aria-label": "Schedule" }, ...[["off", "Off"], ["daily", "Every day"], ["weekly", "Every week"]].map(([v, t]) => h("option", { value: v, text: t, selected: (info?.schedule?.every ?? "off") === v })));
  const day = h("select", { "aria-label": "Day" }, ...DAYS.map((d, i) => h("option", { value: String(i), text: d, selected: (info?.schedule?.weekday ?? 0) === i })));
  const hour = h("select", { "aria-label": "Time" }, ...Array.from({ length: 24 }, (_, i) => h("option", { value: String(i), text: hh(i), selected: (info?.schedule?.hour ?? 10) === i })));
  const consent = h("input", { type: "checkbox" });
  consent.checked = !!info?.schedule?.review_ok;
  const reviewRow = h("label", { class: "ack", hidden: rule.risk !== "review" }, consent,
    h("span", { text: "This is a review rule. I'm fine with it being cleaned without asking me (it still only moves things to the Trash)." }));
  const problem = info?.schedule_problem;
  const sync = () => { day.hidden = every.value !== "weekly"; hour.hidden = every.value === "off"; reviewRow.hidden = rule.risk !== "review" || every.value === "off"; };
  every.addEventListener("change", sync);
  sync();
  const save = h("button", { class: "btn primary", text: "Save", onclick: async () => {
    try {
      const g = parseFloat(gb.value);
      await api.alertSet(rule.id, g > 0 ? Math.round(g * 1e9) : null);
      if (!problem) {
        await api.scheduleSet(rule.id, every.value === "off" ? null : { every: every.value as "daily" | "weekly", hour: +hour.value, weekday: +day.value, review_ok: consent.checked });
      }
      done(); onSaved();
    } catch (e) { toast(String(e), true); }
  } });
  dlg.append(
    h("h3", { id: "tr-title", text: `Track ${rule.name}` }),
    h("div", { class: "re-form" },
      h("label", { class: "field" }, h("span", { class: "nm", text: "Alert me when it reaches" }),
        h("div", { class: "row gap-2" }, gb, h("span", { class: "muted", text: "GB (empty = no alert). At most one notification a day." }))),
      h("div", { class: "field" }, h("span", { class: "nm", text: "Clean it automatically" }),
        problem ? h("span", { class: "muted small", text: problem }) : h("div", { class: "row gap-2 wrap" }, every, day, hour),
        ...(problem ? [] : [h("span", { class: "tertiary small", text: "Runs in the background, moves items to the Trash, skips anything in use, and logs every run. If the Mac is asleep it runs when it wakes." })])),
      reviewRow,
      ...(info?.last_cleaned ? [h("p", { class: "muted small", text: `Last cleaned ${ago(info.last_cleaned)}. ${bytes(info.freed)} freed so far.` })] : [])),
    h("div", { class: "btns" }, h("button", { class: "btn", text: "Cancel", onclick: done }), save));
}
