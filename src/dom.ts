type Child = Node | string | null | undefined | false;
type Props = Record<string, unknown> & { class?: string; text?: string };

/** Tiny hyperscript: h("div", {class:"x", onclick: fn}, child, "text"). */
export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Props = {}, ...kids: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v as string;
    else if (k === "text") el.textContent = v as string;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v as EventListener);
    else if (v === true) el.setAttribute(k, "");
    else el.setAttribute(k, String(v));
  }
  for (const c of kids) if (c) el.append(c);
  return el;
}

export function toast(msg: string, err = false) {
  const t = h("div", { class: "toast" + (err ? " err" : ""), text: msg });
  document.getElementById("toasts")!.append(t);
  setTimeout(() => t.remove(), err ? 6000 : 3200);
}

export const modal = { open: false };

/** The disclosure chevron. It points right, and turns down when its button has aria-expanded="true". */
export function chev(): HTMLSpanElement {
  const s = h("span", { class: "chev", "aria-hidden": "true" });
  s.innerHTML = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 6l6 6-6 6"/></svg>';
  return s;
}

/** Open a `<dialog>` the page controls. `done()` closes and removes it. */
export function openDialog(cls: string, labelId: string): { dlg: HTMLDialogElement; done: () => void } {
  const dlg = h("dialog", { class: cls, "aria-labelledby": labelId });
  const done = () => { modal.open = false; if (dlg.open) dlg.close(); dlg.remove(); };
  dlg.addEventListener("cancel", (e) => { e.preventDefault(); done(); });
  document.body.append(dlg);
  modal.open = true;
  dlg.showModal();
  return { dlg, done };
}

export function confirmDialog(o: { title: string; body: string; ok: string; danger?: boolean }): Promise<boolean> {
  return new Promise((resolve) => {
    const dlg = h("dialog", { "aria-labelledby": "dlg-t" });
    const done = (v: boolean) => { modal.open = false; dlg.close(); dlg.remove(); resolve(v); };
    const cancel = h("button", { class: "btn", text: "Cancel", onclick: () => done(false) });
    const ok = h("button", { class: "btn " + (o.danger ? "danger" : "primary"), text: o.ok, onclick: () => done(true) });
    dlg.append(h("h3", { id: "dlg-t", text: o.title }), h("p", { text: o.body }), h("div", { class: "btns" }, cancel, ok));
    dlg.addEventListener("cancel", (e) => { e.preventDefault(); done(false); });
    document.body.append(dlg);
    modal.open = true;
    dlg.showModal();
    cancel.focus(); // the safe choice is the default
  });
}
