/** Squarified treemap layout (Bruls et al.). Input sizes must be sorted descending. */
export interface Rect { x: number; y: number; w: number; h: number }

export function squarify(sizes: number[], W: number, H: number): Rect[] {
  const out: Rect[] = sizes.map(() => ({ x: 0, y: 0, w: 0, h: 0 }));
  const total = sizes.reduce((a, b) => a + b, 0);
  if (!total || W <= 0 || H <= 0) return out;
  const k = (W * H) / total;
  const s = sizes.map((v) => v * k);
  let x = 0, y = 0, w = W, h = H, i = 0;
  while (i < s.length) {
    const side = Math.min(w, h);
    const worst = (row: number[], sum: number) => {
      const mx = Math.max(...row), mn = Math.min(...row);
      return Math.max((side * side * mx) / (sum * sum), (sum * sum) / (side * side * mn));
    };
    let row = [s[i]], sum = s[i], j = i + 1;
    while (j < s.length) {
      const nr = [...row, s[j]], ns = sum + s[j];
      if (worst(nr, ns) <= worst(row, sum)) { row = nr; sum = ns; j++; } else break;
    }
    const thick = sum / side;
    let off = 0;
    for (let n = 0; n < row.length; n++) {
      const len = row[n] / thick;
      out[i + n] = w >= h ? { x, y: y + off, w: thick, h: len } : { x: x + off, y, w: len, h: thick };
      off += len;
    }
    if (w >= h) { x += thick; w -= thick; } else { y += thick; h -= thick; }
    i = j;
  }
  return out;
}

/** Scrolling treemap: the first band fills the visible height with the largest items, then the rest follow in
 *  further bands below it, each a squarified treemap of its own. A band takes as many items as stay readable
 *  (every cell at least `minW` x `minH`), so small items get a labelled cell instead of a sliver.
 *  Sizes must be sorted descending. Returns the rects and the total height of the canvas. */
export function bandedTreemap(sizes: number[], W: number, H: number, minW = 80, minH = 52, gap = 2): { rects: Rect[]; height: number } {
  const rects: Rect[] = [];
  if (W <= 0 || H <= 0) return { rects: sizes.map(() => ({ x: 0, y: 0, w: 0, h: 0 })), height: H };
  const readable = (rs: Rect[]) => rs.every((r) => r.w >= minW && r.h >= minH);
  const bandH = Math.max(minH * 3, Math.round(H * 0.55));
  let i = 0, y = 0;
  while (i < sizes.length) {
    const first = i === 0;
    let h = first ? H : bandH;
    // the most items that still fit readably (always at least one). Not monotonic: one more item can leave a
    // thin strip that two more would not, so every count up to the cap is tried.
    let n = 1, best = squarify(sizes.slice(i, i + 1), W, h);
    const cap = Math.min(sizes.length - i, Math.floor((W * h) / (minW * minH)));
    for (let m = 2; m <= cap; m++) {
      const next = squarify(sizes.slice(i, i + m), W, h);
      if (readable(next)) { n = m; best = next; }
    }
    // the last band shrinks to what its items need instead of stretching a few cells over a full band
    if (!first && i + n === sizes.length) {
      const prevAvg = rects.length ? rects.reduce((s, r) => s + r.w * r.h, 0) / rects.length : W * h;
      const fit = Math.min(h, Math.max(minH * 1.6, (n * Math.min(prevAvg, (W * h) / n)) / W));
      if (fit < h) {
        const shrunk = squarify(sizes.slice(i, i + n), W, fit);
        if (readable(shrunk)) { h = fit; best = shrunk; } // only when every cell still fits its label
      }
    }
    for (const r of best) rects.push({ x: r.x, y: r.y + y, w: r.w, h: r.h });
    y += h + gap;
    i += n;
  }
  return { rects, height: Math.max(H, y - gap) };
}
