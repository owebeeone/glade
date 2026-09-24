# Gryth Workspace Demo

A share-first demo over the gryth toolchain — **rust + glade + react**. Shared
selection and comparison notes (LWW values), collaborative notes (text CRDT),
and an activity log converge across browser tabs through the local Rust
glade-node. No retrofit: every piece of state is a grip tap with a typed Glade
declaration; Glial mounts it to a Glade client over WebSocket.

```
useGrip components ─ Glial binder/assembly ─ Glade client ─ WS ─ Rust glade-node ─ … other participants
```

## Run (one command)

```
python3 run_demo.py
```

This rebuilds grip-core's `dist`, builds the rust `glade-node`, runs
`pnpm install` for the demo (first run), starts the node on `:9099`, and
runs vite on `:5175`.
Ctrl-C stops everything. Open `http://localhost:5175` in **two tabs** and edit
the **Collaborative notes · text CRDT** surface simultaneously. Put the caret
in the middle in one tab while typing in the other: the remote identity delta
converges without pushing the local caret to the end. The older LWW notes field
remains directly below it as a whole-value comparison. Reloading a tab replays
the operation set from the node. The status dot is `live` when connected and
`offline` otherwise (local edits remain persisted for later resync).

`GLADE_NODE_PORT` and `GLADE_VITE_PORT` pick other ports
(`GLADE_NODE_PORT=9100 python3 run_demo.py`), and the page connects to the port
the node reports. If the node cannot take its port, as when a running desk's
node holds 9099, the runner stops instead of serving a page that would connect
to that other process. `./start-demo.sh` runs the same thing detached, and
`./stop-demo.sh` stops what it started, never whatever else holds the ports.

## Run (manual)

1. **Build grip-core** so its `dist` carries the share feature (gitignored; the
   demo resolves grip-react → grip-core via symlink):
   `(cd ../../grip-core && pnpm build)`
2. **Build + run the node** on 9099:
   `(cd ../node && cargo build --bin glade-node)` then
   `../node/target/debug/glade-node 9099 ../node/target/demo-store`
3. **Install + run the demo**: `pnpm install && pnpm dev`. The page connects to
   the node on 9099, or on `GLADE_NODE_PORT` when it is set
   (`GLADE_NODE_PORT=9100 pnpm dev`).

The wire protocol, folds, and op-hash are the frozen glade contract
(`taut/corpus/glade.*`); the client reproduces them byte-for-byte.
