#!/usr/bin/env bash
# start-demo.sh — start the gryth/glade share-first demo.
#
# Brings up the whole rust + glade + react toolchain via run_demo.py:
# rebuilds grip-core's dist (share feature), builds + starts the rust
# glade-node on :9099, and runs the vite dev server on :5175.
#
# Runs DETACHED; launcher pid -> .demo/run.pid, output -> .demo/run.log.
# Stop with: ./stop-demo.sh
# Open http://localhost:5175/ in TWO tabs (try ?user=alice / ?user=bob).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_DIR="$HERE/.demo"
PIDFILE="$RUN_DIR/run.pid"
LOG="$RUN_DIR/run.log"
NODE_PORT="${GLADE_NODE_PORT:-9099}"
VITE_PORT="${GLADE_VITE_PORT:-5175}"
mkdir -p "$RUN_DIR"
cd "$HERE"   # run from the demo dir regardless of caller's cwd

if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null; then
  echo "glade demo already running (pid $(cat "$PIDFILE")). Run ./stop-demo.sh first." >&2
  exit 1
fi

echo "Starting glade demo — node :$NODE_PORT, vite :$VITE_PORT (rebuilds grip-core; first run ~30s)…"
GLADE_NODE_PORT="$NODE_PORT" GLADE_VITE_PORT="$VITE_PORT" \
  nohup python3 "$HERE/run_demo.py" >"$LOG" 2>&1 &
echo $! >"$PIDFILE"

# Wait for vite to come up (build can take a while on a cold start).
for _ in $(seq 1 90); do
  if lsof -nP -iTCP:"$VITE_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "Ready → http://localhost:$VITE_PORT/   (open two tabs; ?user=alice / ?user=bob, ?doc=N)"
    echo "Logs:   $LOG"
    exit 0
  fi
  sleep 1
done
echo "Started (pid $(cat "$PIDFILE")) but vite not listening on :$VITE_PORT yet — check $LOG" >&2
exit 0
