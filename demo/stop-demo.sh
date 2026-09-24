#!/usr/bin/env bash
# stop-demo.sh — stop the gryth/glade share-first demo started by ./start-demo.sh.
#
# Sends SIGTERM to the run_demo.py launcher recorded in .demo/run.pid; the
# launcher stops the vite server and the glade-node it started, then exits.
# Nothing is stopped by port: another process on the demo's ports (a running
# desk's node holds 9099) is not the demo's to stop.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PIDFILE="$HERE/.demo/run.pid"

PID="$(cat "$PIDFILE" 2>/dev/null || true)"
rm -f "$PIDFILE"
# The pid names the launcher only while it runs run_demo.py: a stale pid may
# belong to another process by now.
COMMAND=""
if [ -n "$PID" ]; then
  COMMAND="$(ps -p "$PID" -o command= 2>/dev/null || true)"
fi
case "$COMMAND" in
  *run_demo.py*) ;;
  *)
    echo "glade demo was not running."
    exit 0
    ;;
esac

kill "$PID"
for _ in $(seq 1 50); do
  if ! kill -0 "$PID" 2>/dev/null; then
    echo "glade demo stopped."
    exit 0
  fi
  sleep 0.1
done
echo "the launcher (pid $PID) is still running; see $HERE/.demo/run.log" >&2
exit 1
