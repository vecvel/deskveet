#!/bin/bash
# Release build. Signs with the "Deskveet Development" certificate when it is in your keychain, so macOS
# keeps permissions such as Full Disk Access across rebuilds. Without it the build is ad-hoc signed and
# macOS treats every build as a new app.
set -e
cd "$(dirname "$0")/.."
IDENTITY="Deskveet Development"
if security find-identity -p codesigning 2>/dev/null | grep -q "\"$IDENTITY\""; then
  echo "Signing with \"$IDENTITY\""
  export APPLE_SIGNING_IDENTITY="$IDENTITY"
else
  echo "No \"$IDENTITY\" certificate found: building ad-hoc signed (permissions reset on every build)."
fi
pnpm tauri build "$@"
codesign -dv src-tauri/target/release/bundle/macos/Deskveet.app 2>&1 | grep -E "^(Identifier|Authority|Signature)=" || true
