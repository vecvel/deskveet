/** Overlay scrollbars. WebKit only repaints styled native scrollbars on hover, so the native ones are
 *  hidden and a thin thumb is drawn over any scroller while it is being scrolled (and while dragged). */
const HIDE_AFTER_MS = 900;
const MIN_THUMB = 28;

interface Bar { thumb: HTMLElement; timer: number; dragging: boolean }
const bars = new WeakMap<HTMLElement, Bar>();

function layout(el: HTMLElement, bar: Bar): boolean {
  const { clientHeight: ch, scrollHeight: sh } = el;
  if (sh <= ch + 1 || !el.isConnected) return false;
  const r = el.getBoundingClientRect();
  const h = Math.max(MIN_THUMB, (ch * ch) / sh);
  const top = r.top + (el.scrollTop / (sh - ch)) * (ch - h);
  // runtime geometry: the only inline styles here
  bar.thumb.style.cssText = `top:${top}px;left:${r.right - 9}px;height:${h}px`;
  return true;
}

function barFor(el: HTMLElement): Bar {
  let bar = bars.get(el);
  if (bar && bar.thumb.isConnected) return bar;
  const thumb = document.createElement("div");
  thumb.className = "sb-thumb";
  // Top-layer dialogs paint above the page, so a dialog's thumb must live inside the dialog.
  (el.closest("dialog") ?? document.body).append(thumb);
  bar = { thumb, timer: 0, dragging: false };
  bars.set(el, bar);
  const b = bar;
  thumb.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    thumb.setPointerCapture(e.pointerId);
    b.dragging = true;
    const y0 = e.clientY, top0 = el.scrollTop;
    const ratio = (el.scrollHeight - el.clientHeight) / Math.max(1, el.clientHeight - thumb.offsetHeight);
    const move = (m: PointerEvent) => { el.scrollTop = top0 + (m.clientY - y0) * ratio; };
    const up = () => { b.dragging = false; thumb.removeEventListener("pointermove", move); show(el); };
    thumb.addEventListener("pointermove", move);
    thumb.addEventListener("pointerup", up, { once: true });
    thumb.addEventListener("pointercancel", up, { once: true });
  });
  return bar;
}

function show(el: HTMLElement) {
  const bar = barFor(el);
  if (!layout(el, bar)) { bar.thumb.classList.remove("on"); return; }
  bar.thumb.classList.add("on");
  clearTimeout(bar.timer);
  bar.timer = window.setTimeout(function hide() {
    if (bar.dragging) { bar.timer = window.setTimeout(hide, HIDE_AFTER_MS); return; }
    bar.thumb.classList.remove("on");
    if (!el.isConnected) bar.thumb.remove();
  }, HIDE_AFTER_MS);
}

export function autoHideScrollbars() {
  document.addEventListener("scroll", (e) => {
    const el = e.target instanceof HTMLElement ? e.target : null;
    if (el) show(el);
  }, { capture: true, passive: true });
}
