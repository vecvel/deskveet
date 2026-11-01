#!/bin/bash
# Launch the release app, let it idle, and print the physical footprint of the app + its WebKit helpers.
# usage: scripts/measure.sh [idle-seconds]
APP="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/target/release/bundle/macos/Deskveet.app"
IDLE=${1:-20}
pkill -x deskveet 2>/dev/null; sleep 1
webkit_pids() { pgrep -f "com.apple.WebKit.(WebContent|GPU|Networking)" | sort; }
BEFORE=$(webkit_pids)
open "$APP"
sleep "$IDLE"
PID=$(pgrep -x deskveet | head -1)
[ -z "$PID" ] && { echo "app not running"; exit 1; }
mb() { footprint -p "$1" 2>/dev/null | grep -m1 Footprint | sed -E 's/.*Footprint: ([0-9.]+) (MB|KB|GB).*/\1 \2/' | awk '{ if ($2=="KB") printf "%.1f", $1/1024; else if ($2=="GB") printf "%.1f", $1*1024; else printf "%.1f", $1 }'; }
sum=$(mb "$PID"); echo "deskveet (rust)       $sum MB"
# WebKit helpers that did not exist before the launch belong to this app.
for pid in $(comm -13 <(echo "$BEFORE") <(webkit_pids)); do
  n=$(ps -o comm= -p "$pid" | sed 's#.*/##; s/com.apple.WebKit.//'); m=$(mb "$pid"); echo "webkit $n  $m MB"; sum=$(echo "$sum + $m" | bc)
done
echo "TOTAL                 $sum MB"
