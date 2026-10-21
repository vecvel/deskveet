#!/bin/bash
# Release build: one universal .app (Apple Silicon + Intel, macOS 13+) and a .dmg.
# Pass --native to build only for this Mac's architecture (faster, smaller); other args go to `tauri build`.
set -e
cd "$(dirname "$0")/.."

TARGET="universal-apple-darwin"
if [ "$1" = "--native" ]; then TARGET=""; shift; fi

if [ -n "$TARGET" ]; then
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target list --installed | grep -qx "$t" || rustup target add "$t"
  done
fi

IDENTITY="Deskveet Development"
if security find-identity -p codesigning 2>/dev/null | grep -q "\"$IDENTITY\""; then
  echo "Signing with \"$IDENTITY\""; export APPLE_SIGNING_IDENTITY="$IDENTITY"
else echo "No \"$IDENTITY\" certificate found: building ad-hoc signed (permissions reset on every build)."; fi

if [ -n "$TARGET" ]; then
  pnpm tauri build --target "$TARGET" "$@"
  OUT="src-tauri/target/$TARGET/release/bundle"
else
  pnpm tauri build "$@"
  OUT="src-tauri/target/release/bundle"
fi

APP="$OUT/macos/Deskveet.app"
codesign -dv "$APP" 2>&1 | grep -E "^(Identifier|Authority|Signature)=" || true
echo "Architectures: $(lipo -archs "$APP/Contents/MacOS/deskveet")"
echo "Minimum macOS: $(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist")"
du -sh "$APP" "$OUT"/dmg/*.dmg 2>/dev/null || true
