const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** base 1000 for disks (matches Finder), base 1024 for RAM (matches Activity Monitor). */
export function bytes(n: number, base: 1000 | 1024 = 1000): string {
  if (!isFinite(n) || n <= 0) return "0 B";
  let i = 0;
  while (n >= base && i < UNITS.length - 1) { n /= base; i++; }
  const d = i === 0 ? 0 : n >= 100 ? 0 : n >= 10 ? 1 : 2;
  return `${n.toFixed(d)} ${UNITS[i]}`;
}
export const ram = (n: number) => bytes(n, 1024);

/** Split into number + unit for big headline numbers. */
export function bytesParts(n: number, base: 1000 | 1024 = 1000): [string, string] {
  const s = bytes(n, base).split(" ");
  return [s[0], s[1]];
}

export const pct = (n: number, d = 0) => `${n.toFixed(d)}%`;

export function uptime(sec: number): string {
  const d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600), m = Math.floor((sec % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

export const env = { home: "" };

export function tilde(p: string): string {
  return env.home && p.startsWith(env.home) ? "~" + p.slice(env.home.length) : p;
}

export const num = (n: number) => n.toLocaleString("en-US");

export function ago(unixSecs: number): string {
  const d = Math.max(0, Date.now() / 1000 - unixSecs);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
}

export function date(unixSecs: number): string {
  if (!unixSecs) return "-";
  return new Date(unixSecs * 1000).toLocaleDateString("en-GB", { day: "2-digit", month: "short", year: "numeric" });
}

export const secs = (ms: number) => (ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)} s`);
