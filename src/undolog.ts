import { api, UndoEntry } from "./api";
import { h, openDialog, toast } from "./dom";
import { bytes, tilde } from "./fmt";

export async function openUndoLog(onChange?: () => void): Promise<void> {
  const { dlg, done } = openDialog("wide", "ul-title");
  const list = h("div", { class: "pv-list" });
  const foot = h("div", { class: "btns" });
  dlg.append(h("h3", { id: "ul-title", text: "Undo log" }), h("p", { class: "muted", id: "ul-sub" }), list, foot);

  async function load() {
    let entries: UndoEntry[];
    try { entries = await api.undoList(); } catch (e) { toast(String(e), true); return; }
    dlg.querySelector("#ul-sub")!.textContent = entries.length ? `${entries.length} recorded action${entries.length === 1 ? "" : "s"}, newest first.` : "Nothing has been cleaned yet.";
    list.replaceChildren(...entries.slice(0, 300).map((e) => {
      const canRestore = e.action === "trash" && e.status === "ok" && !e.restored && !!e.trash_path;
      return h("div", { class: "pv-row" },
        h("div", { class: "pv-main" },
          h("div", { class: "row", style: "gap:8px" }, h("b", { text: e.label }),
            h("span", { class: `pill ${e.status === "ok" ? "ok" : "bad"}`, text: e.restored ? "restored" : `${e.action}${e.status === "ok" ? "" : " failed"}` }),
            h("span", { class: "muted", text: e.at.replace("T", " ").replace("Z", " UTC") })),
          e.path ? h("div", { class: "path", title: e.path, text: tilde(e.path) }) : null,
          e.command ? h("code", { class: "cmd", text: e.command.join(" ") }) : null,
          e.error ? h("div", { class: "why bad", text: e.error }) : null),
        h("div", { class: "act2" },
          h("div", { class: "sz", text: e.bytes ? bytes(e.bytes) : "—" }),
          canRestore ? h("button", { class: "btn sm", text: "Restore", onclick: async () => {
            try { const p = await api.undoRestore(e.id); toast(`Restored to ${tilde(p)}`); onChange?.(); await load(); }
            catch (err) { toast(String(err), true); }
          } }) : null));
    }));
  }

  foot.append(
    h("button", { class: "btn", text: "Open Trash", onclick: () => void api.openTrash() }),
    h("button", { class: "btn", text: "Clear log", onclick: async () => { await api.undoClear(); await load(); } }),
    h("button", { class: "btn primary", text: "Close", onclick: done }));
  await load();
}
