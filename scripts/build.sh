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

# Signing, best first: APPLE_SIGNING_IDENTITY if set (CI), a Developer ID Application certificate (can be
# notarised and shared), the free self-signed "Deskveet Development" one, or ad-hoc. A stable signature keeps
# Full Disk Access granted across rebuilds. Notarisation runs too when APPLE_API_ISSUER, APPLE_API_KEY and
# APPLE_API_KEY_PATH are set (an App Store Connect API key).
if [ -z "$APPLE_SIGNING_IDENTITY" ]; then
  IDS=$(security find-identity -v -p codesigning 2>/dev/null)
  DEVID=$(echo "$IDS" | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"')
  if [ -n "$DEVID" ]; then export APPLE_SIGNING_IDENTITY="$DEVID"
  elif echo "$IDS" | grep -q '"Deskveet Development"'; then export APPLE_SIGNING_IDENTITY="Deskveet Development"
  fi
fi
if [ -n "$APPLE_SIGNING_IDENTITY" ]; then echo "Signing with \"$APPLE_SIGNING_IDENTITY\""
else echo "No signing certificate found: building ad-hoc signed (permissions reset on every build)."; fi

if [ -n "$TARGET" ]; then
  pnpm tauri build --target "$TARGET" "$@"
  OUT="src-tauri/target/$TARGET/release/bundle"
else
  pnpm tauri build "$@"
  OUT="src-tauri/target/release/bundle"
fi

APP="$OUT/macos/Deskveet.app"
codesign -dv "$APP" 2>&1 | grep -E "^(Identifier|Authority|Signature|TeamIdentifier)=" | head -4 || true
echo "Architectures: $(lipo -archs "$APP/Contents/MacOS/deskveet")"
echo "Minimum macOS: $(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist")"
du -sh "$APP" "$OUT"/dmg/*.dmg 2>/dev/null || true
