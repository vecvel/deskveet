import { api, About } from "../api";
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

export function settingsView(): View {
  let about: About | null = null;
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

  function render() {
    const theme = store.get<Theme>("theme", "system");
    const rel = releaseSeconds();
    box.replaceChildren(
      h("section", { class: "card" }, h("h2", { text: "Appearance" }),
        row("Theme", "Follows macOS by default.", group<Theme>("Theme", "theme", theme, [["system", "System"], ["light", "Light"], ["dark", "Dark"]], applyTheme))),
      h("section", { class: "card" }, h("h2", { text: "Memory" }),
        row("Close the window when hidden", "The window is what costs memory (about 70 MB in WebKit helper processes). After this long out of sight it is closed; Deskveet keeps running and reopens instantly from the Dock.",
          group<number>("Release delay", "release", rel, [[30, "30 s"], [60, "1 min"], [300, "5 min"], [0, "Never"]], () => {}))),
      h("section", { class: "card" }, h("h2", { text: "Data on this Mac" }),
        row("Scan cache", about ? `${bytes(about.cache_bytes)} in ${tilde(about.cache_dir)}. Safe to delete; the next scan is just slower.` : "…",
          h("button", { class: "btn", text: "Clear scan cache", onclick: async () => { const n = await api.clearScanCache(); toast(`Removed ${n} cache file${n === 1 ? "" : "s"}`); about = await api.about(); render(); } })),
        row("Undo log", about ? tilde(about.undo_log) : "…",
          h("button", { class: "btn", text: "Show in Finder", onclick: () => about && void api.reveal(about.undo_log).catch((e) => toast(String(e), true)) }))),
      h("section", { class: "card" }, h("h2", { text: "Privacy" }),
        h("p", { class: "facts" }, "Deskveet works entirely offline. It has no network code, no telemetry and no accounts, and the app\u2019s content security policy forbids every network connection. Cleanup tools you choose to run (for example brew or npm) are separate programs and behave as they always do."),
        about ? h("p", { class: "muted small", text: `Version ${about.version} · ${about.rules_count} cleanup rules (${about.rules_version})` }) : null),
    );
  }

  return {
    mount(root) {
      box = h("div", { class: "cleanup" });
      root.append(header("Settings"), box);
      render();
      void api.about().then((a) => { about = a; render(); }).catch(() => {});
    },
  };
}
