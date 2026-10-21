# Deskveet

A light, fast, privacy-first disk and system cleanup app for macOS (Rust + Tauri 2, plain TypeScript UI).
Fully offline: no network code, no telemetry, no accounts. The content security policy forbids every
connection, and nothing in the dependency list opens a socket.

## What it does

- **Overview**: disks (used / purgeable / free, warning above 90%), RAM (app, wired, compressed, swap, pressure),
  CPU per core and load, top processes. Live while the window is visible, silent when it is hidden.
- **Processes**: sortable, filterable, "Open in Finder", Quit / Force Quit with confirmation. System, root,
  other users' processes and a hard-coded list (`kernel_task`, `WindowServer`, `launchd`, ...) are never offered.
- **Storage**: parallel scan of any folder, sizes per folder, drill-down, treemap, cached on disk with an
  incremental re-scan. Every row has Reveal in Finder, Open, Copy path, Quick Look and Move to Trash.
- **Cleanup**: 33 rules in [`rules/macos.toml`](rules/macos.toml) (Xcode, simulators, Android, Flutter, Node, Rust,
  Go, Python, Homebrew, CocoaPods, browsers, app caches, logs, Trash). Each rule says what happens if you delete
  it and how it comes back. Always preview first, then confirm. Dry run, undo log, Restore from the log.
- **Project artifacts**: `node_modules`, `target`, `build`, `.dart_tool`, `.gradle`, `__pycache__`, `DerivedData`
  below folders you choose, grouped by project with idle time and one-click "select stale".

## Safety model (`src-tauri/src/safety.rs`)

Nothing is acted on unless it passes **both** gates, on the real path (symlinked parents resolved):

1. **Allowlist**: matched by the rule that claims it (glob, or folder name + marker file + chosen root), or
   picked explicitly by the user (always to the Trash).
2. **Blocklist** (always wins): `/`, `/System`, `/usr`, `/Library`, your home, Desktop/Documents/Pictures/Movies/Music
   roots, iCloud Drive, Photos/Music/TV libraries, every `.git` folder, keychains, `~/.ssh`, `~/.gnupg`, Mail,
   Messages, Safari data, group containers, ...

The UI never sends paths for rule-based cleaning, only ids the engine produced. Running browsers block their
cache rules, booted simulators are never touched, open files and running tools need an explicit tick, and the
Trash is only erased after a second confirmation. No sudo, ever: rules that need admin show the command instead.

## Memory and size

| | |
|---|---|
| `.app` bundle | about 2.5 MB (Rust binary 2.6 MB, frontend 12 KB JS + 9 KB CSS) |
| Window open | Rust process about 25 MB plus WebKit helpers about 70 MB (WKWebView is the floor) |
| Window closed | Rust process only; the window is released after it has been hidden for a while (Settings) |
| Scan of 3.4M files | about 36 MB peak, 44 s cold, 12 to 18 s incremental |

Measure it yourself: `scripts/measure.sh` (footprint of the app plus its WebKit helpers).

## Develop

```bash
pnpm install
pnpm tauri dev                 # run
cd src-tauri && cargo test     # 54 tests, all against temp-dir fixtures with a fake system layer
pnpm tauri build --bundles app # release .app (add dmg for a disk image); unsigned for now
```

Dev helpers in `src-tauri/examples/` run the real engine read-only (`cleanup_dry`, `scan_bench`,
`artifacts_dry`) or against a throwaway file you create (`trash_smoke`, `artifact_smoke`).

macOS asks for permission the first time an unsigned build reads Desktop, Documents or Downloads, and the
Trash can only be listed with Full Disk Access (Deskveet links straight to that settings pane).

## Portability

OS-specific code sits behind the `Platform` and `ProcessInfo` traits (`src-tauri/src/platform`) and the `Sys`
trait for side effects (`src-tauri/src/sys.rs`). A Windows or Linux port adds a module and a rules file.
