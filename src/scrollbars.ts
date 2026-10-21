/** Show a scroller's thumb while it is being scrolled, hide it shortly after it stops. */
const HIDE_AFTER_MS = 900;
const timers = new WeakMap<Element, number>();

function hide(el: HTMLElement) {
  el.classList.remove("is-scrolling");
  // WebKit only repaints a custom scrollbar on layout; nudge it so the thumb really disappears.
  const prev = el.style.overflowY;
  el.style.overflowY = "hidden";
  void el.offsetHeight;
  el.style.overflowY = prev;
}

export function autoHideScrollbars() {
  document.addEventListener("scroll", (e) => {
    const el = e.target instanceof HTMLElement ? e.target : document.scrollingElement as HTMLElement | null;
    if (!el) return;
    el.classList.add("is-scrolling");
    clearTimeout(timers.get(el));
    timers.set(el, window.setTimeout(() => hide(el), HIDE_AFTER_MS));
  }, { capture: true, passive: true });
}
