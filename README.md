# Deskveet

A light, fast, privacy-first disk and system cleanup app for macOS (Rust + Tauri 2, plain TypeScript UI).
Fully offline: no network code, no telemetry, no accounts. The content security policy forbids every
connection, and nothing in the dependency list opens a socket.

## What it does

- **Overview**: disks (used / purgeable / free, warning above 90%), RAM (app, wired, compressed, swap, pressure),
  CPU per core and load, top processes. Live while the window is visible, silent when it is hidden.
- **Processes**: sortable, filterable, "Open in Finder", Quit / Force Quit with confirmation. System, root,
  other users' processes and a hard-coded list (`kernel_task`, `WindowServer`, `launchd`, ...) are never offered.
- **Apps**: running apps like the macOS Force Quit window, with icon, memory and CPU (summed over every process
  macOS holds the app responsible for), and storage (the app bundle plus its caches, support files, web data and
  saved state). Quit asks the app like Cmd+Q does; Force Quit stops it after a confirmation. Select several to
  quit them together. Finder, the Dock and Deskveet itself are protected.
- **Storage**: parallel scan of any folder, sizes per folder, drill-down, treemap, cached on disk with an
  incremental re-scan. Every row has Reveal in Finder, Open, Copy path, Quick Look and Move to Trash.
- **Cleanup**: 33 rules in [`rules/macos.toml`](rules/macos.toml) (Xcode, simulators, Android, Flutter, Node, Rust,
  Go, Python, Homebrew, CocoaPods, browsers, app caches, logs, Trash). Each rule says what happens if you delete
  it and how it comes back. Always preview first, then confirm. Dry run, undo log, Restore from the log.
- **Rules on/off**: Settings → Cleanup rules lists every rule with a switch; rules that are off are not scanned
  and their items are refused by the preview.
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

## Design system

All styling lives in `src/styles/`: `tokens.css` (colour, text levels, the single near-transparent border
token, type, spacing, radii, themes), `base.css` (reset, typography, utilities) and `components.css`.
Restyle the app by editing `tokens.css`. System font everywhere, a warm charcoal / parchment palette with a
terracotta accent, light and dark follow macOS (or Settings → Theme). Below 920 px the sidebar becomes an
icon rail.

**Interaction (a deliberate choice):** controls have no hover, focus-ring or press styling; only functional
states (selected, disabled, invalid) are shown. Nothing depends on hover. This hurts keyboard users, who get no
visible focus; if you want it back, add one `:focus-visible` rule to `base.css`.

## Supported Macs

macOS 13 Ventura or later, on Apple Silicon and Intel. `scripts/build.sh` makes one universal `.app` that
contains both. The CSS uses only literal colours (no `color-mix()`), so it looks the same in every WebKit that
macOS 13 can have. Tool paths are found on each Mac rather than assumed: Homebrew under `/opt/homebrew` or
`/usr/local`, Xcode through `xcode-select`, and caches through their environment variables or the tool itself
(`src-tauri/src/locate.rs`).

Sharing a build: copies that aren't signed with a Developer ID are stopped by Gatekeeper the first time. To open
one, right-click the app, choose Open, then confirm. Signing with a Developer ID and notarising (needs a paid
Apple developer account) removes this step; the build works without it.

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
cd src-tauri && cargo test     # 87 tests, all against temp-dir fixtures with a fake system layer
scripts/build.sh               # universal release .app + .dmg (--native: this Mac only), signed with "Deskveet Development" if present
```

Dev helpers in `src-tauri/examples/` run the real engine read-only (`cleanup_dry`, `scan_bench`,
`artifacts_dry`) or against a throwaway file you create (`trash_smoke`, `artifact_smoke`).

macOS ties privacy permissions (Full Disk Access, Desktop/Documents access) to the app's code signature. An
ad-hoc signed build looks like a new app every time, so a permission you granted to an earlier build
silently stops applying. Create a free self-signed certificate once and `scripts/build.sh` will use it:

1. Keychain Access → Certificate Assistant → Create a Certificate…
2. Name `Deskveet Development`, Identity Type *Self-Signed Root*, Certificate Type *Code Signing* → Create.
3. Run `scripts/build.sh`, then in System Settings → Privacy & Security → Full Disk Access remove Deskveet
   with − and add the new build once. From then on it stays granted across rebuilds.

Deskveet checks access live (`fda_status`) whenever the window comes back to the front, so the banners
reflect the build that is actually running.

## Portability

OS-specific code sits behind the `Platform` and `ProcessInfo` traits (`src-tauri/src/platform`) and the `Sys`
trait for side effects (`src-tauri/src/sys.rs`). A Windows or Linux port adds a module and a rules file.
