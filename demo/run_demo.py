#!/usr/bin/env python3
"""Start the gryth workspace demo end to end (GLP-0005 P3.S4).

One command brings up the whole rust + glade + react toolchain:
  1. rebuild grip-core's dist so the share feature reaches the demo
     (grip-react -> grip-core is a symlink; dist is gitignored)
  2. build the rust glade-node
  3. pnpm install the demo (first run only)
  4. start the glade-node (background, port 9099) and wait until it listens;
     if it cannot take its port, stop there
  5. run the vite dev server (foreground), telling the page the node's port

Ctrl-C, or the SIGTERM that ./stop-demo.sh sends, stops vite and tears the
node down. Open the dev URL in two tabs to see shared selection/notes (lww)
and the activity log (log) converge.

The page connects to the node this runner started, on the port the node
reports (vite.config.ts reads it from GLADE_NODE_PORT). If another process
holds that port -- a running desk's node holds 9099 -- the node exits and the
runner stops, rather than serve a page that would connect to that process.

Usage:  python3 run_demo.py            # ports: node 9099, vite 5175
        GLADE_NODE_PORT=9100 python3 run_demo.py
"""

from __future__ import annotations

import atexit
import os
import signal
import subprocess
import sys
import threading
from pathlib import Path
from typing import IO

HERE = Path(__file__).resolve().parent          # glade/demo
GLADE = HERE.parent                              # glade
ROOT = GLADE.parent                              # the workspace root
GRIP_CORE = ROOT / "grip-core"
NODE_DIR = GLADE / "node"
NODE_BIN = NODE_DIR / "target" / "debug" / "glade-node"

NODE_PORT = os.environ.get("GLADE_NODE_PORT", "9099")
VITE_PORT = os.environ.get("GLADE_VITE_PORT", "5175")
STORE = NODE_DIR / "target" / "demo-store"

# Each started process, and whether it leads a process group of its own: vite
# runs under pnpm in a group of its own, so one signal reaches both.
_procs: list[tuple[subprocess.Popen, bool]] = []


def run(cmd: list[str], cwd: Path) -> None:
    print(f"\n\033[36m+ ({cwd.name}) {' '.join(cmd)}\033[0m")
    subprocess.run(cmd, cwd=str(cwd), check=True)


def cleanup() -> None:
    for p, group in reversed(_procs):
        if p.poll() is None:
            if group:
                os.killpg(p.pid, signal.SIGTERM)
            else:
                p.terminate()
            try:
                p.wait(timeout=3)
            except subprocess.TimeoutExpired:
                if group:
                    os.killpg(p.pid, signal.SIGKILL)
                else:
                    p.kill()


def stop(signum: int, _frame: object) -> None:
    """End the launcher through SystemExit, so that cleanup runs."""
    raise SystemExit(128 + signum)


def forward(stream: IO[str]) -> None:
    for line in stream:
        print(line, end="", flush=True)


def start_node() -> str | None:
    """Start the glade-node; return the port its `listening <port>` line
    reports, or None if it exits first, as it does when the port is taken."""
    print(f"\n\033[32m+ starting glade-node on :{NODE_PORT} (store {STORE})\033[0m")
    node = subprocess.Popen(
        [str(NODE_BIN), NODE_PORT, str(STORE)], stdout=subprocess.PIPE, text=True
    )
    _procs.append((node, False))
    assert node.stdout is not None
    for line in node.stdout:
        print(line, end="", flush=True)
        if line.startswith("listening "):
            # Keep draining its stdout, so a full pipe never stalls the node.
            threading.Thread(target=forward, args=(node.stdout,), daemon=True).start()
            return line.split()[1]
    node.wait()
    return None


def main() -> int:
    # Line by line, so a log (./start-demo.sh writes one) keeps these lines in
    # order with the output of the commands they announce.
    sys.stdout.reconfigure(line_buffering=True)
    atexit.register(cleanup)
    signal.signal(signal.SIGTERM, stop)
    # Under nohup (./start-demo.sh) a hangup stays ignored, as nohup asked.
    if signal.getsignal(signal.SIGHUP) is not signal.SIG_IGN:
        signal.signal(signal.SIGHUP, stop)

    # 1. grip-core dist (carries the GQ-5 share feature; gitignored)
    if not (GRIP_CORE / "node_modules").exists():
        run(["pnpm", "install"], GRIP_CORE)
    run(["pnpm", "run", "build"], GRIP_CORE)

    # 2. the rust glade-node
    run(["cargo", "build", "--offline", "--bin", "glade-node"], NODE_DIR)

    # 3. demo deps (first run)
    if not (HERE / "node_modules").exists():
        run(["pnpm", "install"], HERE)

    # 4. glade-node in the background, listening before the page is served
    port = start_node()
    if port is None:
        print(
            f"\n\033[31mglade-node did not start on :{NODE_PORT}. If another process"
            f" holds that port (a running desk's node holds 9099), pick a free one:"
            f" GLADE_NODE_PORT=9100 python3 run_demo.py\033[0m",
            file=sys.stderr,
        )
        return 1

    # 5. vite dev (foreground), told the node's port. No `--` before the flags:
    # pnpm passes a `--` on to the script, and vite ignores every flag after
    # one, so it would fall back to its default port.
    cmd = ["pnpm", "run", "dev", "--port", VITE_PORT, "--strictPort"]
    print(f"\n\033[32m+ vite dev on :{VITE_PORT}, node :{port} — open it in two tabs\033[0m")
    print(f"\033[36m+ ({HERE.name}) GLADE_NODE_PORT={port} {' '.join(cmd)}\033[0m")
    vite = subprocess.Popen(
        cmd, cwd=str(HERE), env=dict(os.environ, GLADE_NODE_PORT=port), start_new_session=True
    )
    _procs.append((vite, True))
    try:
        vite.wait()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
