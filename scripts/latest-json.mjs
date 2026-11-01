// Writes latest.json, the file installed copies of Deskveet read to update themselves (Tauri updater format).
// File names carry no version, so the README can link to releases/latest/download/<file>.
// Usage: node scripts/latest-json.mjs <folder with the release files and their .sig files> <tag>
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const [dir, tag] = process.argv.slice(2);
const version = tag.replace(/^v/, "");
const files = readdirSync(dir);
const base = `https://github.com/vecvel/deskveet/releases/download/${tag}`;

function entry(name) {
  if (!files.includes(name) || !files.includes(`${name}.sig`)) throw new Error(`missing ${name} or its .sig`);
  return { signature: readFileSync(join(dir, `${name}.sig`), "utf8").trim(), url: `${base}/${name}` };
}

const mac = entry(`Deskveet-universal.app.tar.gz`);
const manifest = {
  version,
  notes: `Deskveet ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    // one universal app serves both Mac architectures
    "darwin-aarch64": mac,
    "darwin-x86_64": mac,
    "windows-x86_64": entry(`Deskveet-x64-setup.exe`),
    "windows-aarch64": entry(`Deskveet-arm64-setup.exe`),
    // only the AppImage updates itself; the .deb is updated by installing the newer package
    "linux-x86_64": entry(`Deskveet-amd64.AppImage`),
    "linux-aarch64": entry(`Deskveet-arm64.AppImage`),
  },
};
process.stdout.write(JSON.stringify(manifest, null, 2) + "\n");
