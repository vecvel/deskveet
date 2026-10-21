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
