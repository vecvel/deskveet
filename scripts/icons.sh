#!/bin/bash
# Regenerate every icon from the mark in scripts/logo.py. Needs rsvg-convert (brew install librsvg).
set -e
cd "$(dirname "$0")/.."
python3 scripts/logo.py > /dev/null
OUT=src-tauri/icons
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

png() { rsvg-convert -w "$2" -h "$2" "$1" -o "$3"; }

png assets/icon.svg 1024 assets/icon.png
png assets/icon.svg 32  "$OUT/32x32.png"
png assets/icon.svg 128 "$OUT/128x128.png"
png assets/icon.svg 256 "$OUT/128x128@2x.png"
png assets/icon.svg 512 "$OUT/icon.png"
png assets/tray.svg 36  "$OUT/tray.png"           # 18 pt menu bar icon at 2x

SET="$TMP/icon.iconset"; mkdir "$SET"
for s in 16 32 128 256 512; do
  png assets/icon.svg "$s" "$SET/icon_${s}x${s}.png"
  png assets/icon.svg "$((s * 2))" "$SET/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$SET" -o "$OUT/icon.icns"
for s in 24 48 64; do png assets/icon.svg "$s" "$TMP/ico_$s.png"; done
# .ico with PNG entries (what Windows expects from Vista on; much smaller than bitmaps)
python3 - "$OUT/icon.ico" "$SET/icon_16x16.png" "$TMP/ico_24.png" "$SET/icon_32x32.png" "$TMP/ico_48.png" "$TMP/ico_64.png" "$SET/icon_256x256.png" <<'PY'
import struct, sys
out, files = sys.argv[1], sys.argv[2:]
blobs = [open(f, "rb").read() for f in files]
size = lambda b: struct.unpack(">I", b[16:20])[0]
head = struct.pack("<HHH", 0, 1, len(blobs)); off = 6 + 16 * len(blobs); dirs = b""
for b in blobs:
    w = size(b); dirs += struct.pack("<BBBBHHII", w % 256, w % 256, 0, 0, 1, 32, len(b), off); off += len(b)
open(out, "wb").write(head + dirs + b"".join(blobs))
PY
ls -l "$OUT"
