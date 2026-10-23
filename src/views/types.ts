export interface View {
  /** Build the DOM once. */
  mount(root: HTMLElement): void;
  /** Called on the live timer while visible; omit for static views. */
  tick?(): Promise<void>;
  /** Live refresh period in ms. */
  interval?: number;
  /** Cmd+F */
  find?(): void;
  /** Cmd+R (full = Shift+Cmd+R) */
  refresh?(full?: boolean): void;
  unmount?(): void;
}

export function header(title: string, sub = "", ...extra: (Node | null)[]): HTMLElement {
  const d = document.createElement("div");
  d.className = "head";
  // the window moves when the header's own background or text is dragged; its buttons stay buttons
  d.innerHTML = `<h1 data-tauri-drag-region></h1><span class="sub" data-tauri-drag-region></span><span class="sp" data-tauri-drag-region></span>`;
  d.setAttribute("data-tauri-drag-region", "");
  d.querySelector("h1")!.textContent = title;
  d.querySelector(".sub")!.textContent = sub;
  for (const e of extra) if (e) d.append(e);
  return d;
}
