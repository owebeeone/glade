#!/usr/bin/env python3
"""Start the gryth workspace demo end to end (GLP-0005 P3.S4).

One command brings up the whole rust + glade + react toolchain:
  1. rebuild grip-core's dist so the share feature reaches the demo
     (grip-react -> grip-core is a symlink; dist is gitignored)
  2. build the rust glade-node
  3. npm install the demo (first run only)
  4. start the glade-node (background, port 9099)
  5. run the vite dev server (foreground)

Ctrl-C stops vite and tears the node down. Open the dev URL in two tabs to see
shared selection/notes (lww) and the activity log (log) converge.

Usage:  python3 run_demo.py            # ports: node 9099, vite 5175
        GLADE_NODE_PORT=9100 python3 run_demo.py
"""

from __future__ import annotations

import atexit
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent          # glade/demo
GLADE = HERE.parent                              # glade
ROOT = GLADE.parent                              # glial-dev
GRIP_CORE = ROOT / "grip-core"
NODE_DIR = GLADE / "node"
NODE_BIN = NODE_DIR / "target" / "debug" / "glade-node"

NODE_PORT = os.environ.get("GLADE_NODE_PORT", "9099")
VITE_PORT = os.environ.get("GLADE_VITE_PORT", "5175")
STORE = NODE_DIR / "target" / "demo-store"

_procs: list[subprocess.Popen] = []


def run(cmd: list[str], cwd: Path) -> None:
    print(f"\n\033[36m+ ({cwd.name}) {' '.join(cmd)}\033[0m")
    subprocess.run(cmd, cwd=str(cwd), check=True)


def cleanup() -> None:
    for p in _procs:
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=3)
            except subprocess.TimeoutExpired:
                p.kill()


def main() -> int:
    atexit.register(cleanup)

    # 1. grip-core dist (carries the GQ-5 share feature; gitignored)
    if not (GRIP_CORE / "node_modules").exists():
        run(["npm", "install"], GRIP_CORE)
    run(["npm", "run", "build"], GRIP_CORE)

    # 2. the rust glade-node
    run(["cargo", "build", "--offline", "--bin", "glade-node"], NODE_DIR)

    # 3. demo deps (first run)
    if not (HERE / "node_modules").exists():
        run(["npm", "install"], HERE)

    # 4. glade-node in the background
    print(f"\n\033[32m+ starting glade-node on :{NODE_PORT} (store {STORE})\033[0m")
    _procs.append(subprocess.Popen([str(NODE_BIN), NODE_PORT, str(STORE)]))
    time.sleep(0.5)

    # 5. vite dev (foreground). Note: the demo connects to ws://127.0.0.1:9099.
    print(f"\n\033[32m+ vite dev on :{VITE_PORT} — open it in two tabs\033[0m")
    try:
        run(["npm", "run", "dev", "--", "--port", VITE_PORT, "--strictPort"], HERE)
    except KeyboardInterrupt:
        pass
    except subprocess.CalledProcessError:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
