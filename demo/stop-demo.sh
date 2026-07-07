#!/usr/bin/env bash
# stop-demo.sh — stop the gryth/glade share-first demo started by ./start-demo.sh.
#
# Kills the run_demo.py launcher and frees the node (:9099) and vite (:5175)
# ports — run_demo.py's children outlive a plain kill of the launcher, so we
# also reclaim the ports directly.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PIDFILE="$HERE/.demo/run.pid"
NODE_PORT="${GLADE_NODE_PORT:-9099}"
VITE_PORT="${GLADE_VITE_PORT:-5175}"

stopped=0
if [ -f "$PIDFILE" ]; then
  PID="$(cat "$PIDFILE" 2>/dev/null || true)"
  if [ -n "${PID:-}" ] && kill "$PID" 2>/dev/null; then
    echo "stopped launcher (pid $PID)"; stopped=1
  fi
  rm -f "$PIDFILE"
fi

for port in "$VITE_PORT" "$NODE_PORT"; do
  pids="$(lsof -ti tcp:"$port" 2>/dev/null || true)"
  if [ -n "$pids" ]; then
    kill $pids 2>/dev/null || true
    echo "freed port $port"; stopped=1
  fi
done

[ "$stopped" -eq 1 ] && echo "glade demo stopped." || echo "glade demo was not running."
