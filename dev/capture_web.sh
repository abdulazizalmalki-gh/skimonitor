#!/usr/bin/env bash
# Re-shoot docs/web.png from a demo instance, at the canonical 150x46 grid.
# Needs: a browser with a headless screenshot mode, and the binary built.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT=${PORT:-8399}
CHROME=${CHROME:-google-chrome}
BIN=${BIN:-./target/release/skimonitor}

# window sized so the page's own cell math lands on 150 cols x 46 rows
$BIN --web --demo -p "$PORT" >/tmp/skimonitor-demo.log 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT

sleep 40   # let the sparklines fill; an empty trend is honest but looks broken
"$CHROME" --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage \
  --hide-scrollbars --force-device-scale-factor=1 \
  --screenshot=docs/web.png --window-size=1450,1160 \
  --virtual-time-budget=8000 --timeout=30000 \
  "http://127.0.0.1:$PORT/?static=1"

echo "wrote docs/web.png"
