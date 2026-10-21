import { api, Overview, ProcRow, DiskRow } from "../api";
import { h } from "../dom";
import { bytes, bytesParts, env, pct, ram, uptime } from "../fmt";
import { icons } from "../icons";
import { header, View } from "./types";

const SPARK_N = 60;

export function overviewView(go: (id: string) => void): View {
  const hist: number[] = [];
  let sub!: HTMLElement;
  let disksBox!: HTMLElement;
  let memBig!: HTMLElement, memBar!: HTMLElement, memLegend!: HTMLElement, memPill!: HTMLElement, memSwap!: HTMLElement;
  let cpuBig!: HTMLElement, cpuLoad!: HTMLElement, cores!: HTMLElement, spark!: HTMLCanvasElement;
  let topMem!: HTMLElement, topCpu!: HTMLElement;
  const diskNodes = new Map<string, { root: HTMLElement; bar: HTMLElement; txt: HTMLElement; warn: HTMLElement }>();

  const seg = (cls: string, w: number) => `<i class="${cls}" style="width:${Math.max(0, Math.min(100, w)).toFixed(2)}%"></i>`;
  const dot = (cls: string, label: string, val: string) =>
    `<span style="--dot:var(--${cls})">${label} <b>${val}</b></span>`;

  function renderDisk(d: DiskRow) {
    let n = diskNodes.get(d.mount);
    if (!n) {
      const warn = h("div", { class: "banner", hidden: true });
      const bar = h("div", { class: "bar thick" });
      const txt = h("div", { class: "legend" });
      const root = h("div", { class: "disk" },
        h("div", { class: "row sb" },
          h("div", {}, h("span", { class: "nm", text: d.name }), h("span", { class: "muted", text: `  ${d.mount} · ${d.fs}${d.removable ? " · external" : ""}` })),
          h("div", { class: "big", id: "" })),
        bar, txt, warn);
      disksBox.append(root);
      n = { root, bar, txt, warn };
      diskNodes.set(d.mount, n);
    }
    const [v, u] = bytesParts(d.free + d.purgeable);
    n.root.querySelector<HTMLElement>(".big")!.innerHTML = `${v}<small>${u} available</small>`;
    const hot = d.used_pct >= 90;
    n.bar.innerHTML = seg(hot ? "c-bad" : "c-used", (d.used / d.total) * 100) + (d.purgeable ? seg("c-purge", (d.purgeable / d.total) * 100) : "");
    n.txt.innerHTML = dot("accent", "Used", bytes(d.used)) + (d.purgeable ? dot("accent", "Purgeable", bytes(d.purgeable)) : "") + dot("text-3", "Free", bytes(d.free)) + dot("text-3", "Total", bytes(d.total));
    n.warn.hidden = !hot;
    n.warn.textContent = `${pct(d.used_pct)} full. Free some space soon: macOS gets slow and unstable when a disk is nearly full.`;
  }

  function procList(box: HTMLElement, rows: ProcRow[], kind: "mem" | "cpu") {
    box.replaceChildren(
      ...rows.map((r) => {
        const tr = h("tr", { title: r.path ?? r.name });
        tr.append(h("td", { text: r.name }), h("td", { class: "v", text: kind === "mem" ? ram(r.mem) : pct(r.cpu) }));
        return tr;
      }),
    );
  }

  function drawSpark() {
    const c = spark, dpr = devicePixelRatio || 1;
    const w = c.clientWidth, hgt = c.clientHeight;
    if (c.width !== w * dpr) { c.width = w * dpr; c.height = hgt * dpr; }
    const g = c.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, hgt);
    const css = getComputedStyle(document.documentElement);
    const col = css.getPropertyValue("--accent").trim();
    const ink = css.getPropertyValue("--text").trim();
    g.globalAlpha = 0.1; g.strokeStyle = ink; g.lineWidth = 1;
    g.beginPath(); g.moveTo(0, hgt - 0.5); g.lineTo(w, hgt - 0.5); g.stroke();
    if (hist.length < 2) return;
    const step = w / (SPARK_N - 1), x0 = w - (hist.length - 1) * step;
    g.beginPath();
    hist.forEach((v, i) => { const x = x0 + i * step, y = hgt - 2 - (v / 100) * (hgt - 4); i ? g.lineTo(x, y) : g.moveTo(x, y); });
    g.strokeStyle = col; g.lineWidth = 1.6; g.stroke();
    g.lineTo(w, hgt); g.lineTo(x0, hgt); g.closePath();
    g.globalAlpha = 0.16; g.fillStyle = col; g.fill(); g.globalAlpha = 1;
  }

  function apply(o: Overview) {
    env.home = o.home;
    sub.textContent = `${o.host} · up ${uptime(o.uptime)}`;
    o.disks.forEach(renderDisk);

    const m = o.mem, p = m.total / 100;
    const [mv, mu] = bytesParts(m.used, 1024);
    memBig.innerHTML = `${mv}<small>${mu} of ${ram(m.total)} used</small>`;
    memBar.innerHTML = seg("c-app", m.app / p) + seg("c-wired", m.wired / p) + seg("c-comp", m.compressed / p);
    memLegend.innerHTML = dot("accent", "App", ram(m.app)) + dot("sand", "Wired", ram(m.wired)) + dot("slate", "Compressed", ram(m.compressed)) + dot("text-3", "Cached", ram(m.cached));
    const lvl = { normal: ["ok", "Normal"], warn: ["warn", "Warning"], critical: ["bad", "Critical"] }[m.pressure];
    memPill.replaceChildren("Memory pressure ", h("b", { class: lvl[0], text: lvl[1] }));
    memSwap.textContent = m.swap_total ? `Swap ${ram(m.swap_used)} of ${ram(m.swap_total)}` : "Swap not in use";

    cpuBig.innerHTML = `${o.cpu.overall.toFixed(0)}<small>% · ${o.cpu.cores.length} cores</small>`;
    cpuLoad.textContent = `Load average  ${o.cpu.load.map((x) => x.toFixed(2)).join("  ·  ")}`;
    if (cores.childElementCount !== o.cpu.cores.length) cores.replaceChildren(...o.cpu.cores.map(() => h("i", {}, h("b"))));
    o.cpu.cores.forEach((c, i) => {
      const b = cores.children[i].firstElementChild as HTMLElement;
      b.style.height = `${Math.min(100, c)}%`;
      (cores.children[i] as HTMLElement).title = `Core ${i + 1}: ${c.toFixed(0)}%`;
    });
    hist.push(o.cpu.overall);
    if (hist.length > SPARK_N) hist.shift();
    drawSpark();

    procList(topMem, o.top_mem, "mem");
    procList(topCpu, o.top_cpu, "cpu");
  }

  const view: View = {
    interval: 1500,
    mount(root) {
      // The DOM is rebuilt on every visit: forget rows that belonged to the previous one.
      diskNodes.clear();
      sub = h("span", { class: "sub" });
      const head = header("Overview");
      head.querySelector(".sub")!.replaceWith(sub);
      disksBox = h("div");
      memBig = h("div", { class: "big", text: "…" });
      memBar = h("div", { class: "bar thick" });
      memLegend = h("div", { class: "legend" });
      memPill = h("span", { class: "status" });
      memSwap = h("span", { class: "muted" });
      cpuBig = h("div", { class: "big", text: "…" });
      cpuLoad = h("div", { class: "muted" });
      cores = h("div", { class: "cores" });
      spark = h("canvas", { class: "spark" });
      topMem = h("tbody");
      topCpu = h("tbody");
      const more = (label: string) => {
        const b = h("button", { class: "icon-btn", title: label, "aria-label": label, onclick: () => go("processes") });
        b.innerHTML = icons.chevron;
        return b;
      };
      root.append(
        head,
        h("div", { class: "grid" },
          h("section", { class: "card wide" }, h("h2", { text: "Disks" }), disksBox),
          h("section", { class: "card" }, h("h2", {}, "Memory", h("span", { class: "sp" }), memPill), memBig, memBar, memLegend, h("div", { class: "mt-2" }, memSwap)),
          h("section", { class: "card" }, h("h2", { text: "CPU" }), cpuBig, spark, cores, h("div", { class: "mt-2" }, cpuLoad)),
          h("section", { class: "card" }, h("h2", {}, "Top by memory", h("span", { class: "sp" }), more("View all processes by memory")), h("table", { class: "mini" }, topMem)),
          h("section", { class: "card" }, h("h2", {}, "Top by CPU", h("span", { class: "sp" }), more("View all processes by CPU")), h("table", { class: "mini" }, topCpu)),
        ),
      );
    },
    async tick() { apply(await api.overview()); },
    refresh() { void view.tick!(); },
  };
  return view;
}
