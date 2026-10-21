export interface View {
  /** Build the DOM once. */
  mount(root: HTMLElement): void;
  /** Called on the live timer while visible; omit for static views. */
  tick?(): Promise<void>;
  /** Live refresh period in ms. */
  interval?: number;
  /** Cmd+F */
  find?(): void;
  /** Cmd+R */
  refresh?(): void;
  unmount?(): void;
}

export function header(title: string, sub = "", ...extra: (Node | null)[]): HTMLElement {
  const d = document.createElement("div");
  d.className = "head";
  d.innerHTML = `<h1></h1><span class="sub"></span><span class="sp"></span>`;
  d.querySelector("h1")!.textContent = title;
  d.querySelector(".sub")!.textContent = sub;
  for (const e of extra) if (e) d.append(e);
  return d;
}
