# Gryth Workspace Demo

A share-first demo over the gryth toolchain — **rust + glade + react**. Shared
selection and notes (lww values) and an activity log (append log) converge
across browser tabs through the local rust glade-node. No retrofit: every piece
of state is a grip tap that declares a `share` (GQ-5); the grip-share binder
wires them to a glade client over a websocket to the node.

```
useGrip components ─ grip-share binder ─ glade client ─ WS ─ rust glade-node ─ … other participants
```

## Run

1. **Build grip-core** so its `dist` carries the share feature (it is
   gitignored, and the demo resolves grip-react → grip-core via symlink):
   ```
   (cd ../../grip-core && npm run build)
   ```
2. **Build + run the node** on port 9099:
   ```
   (cd ../node && cargo build --bin glade-node)
   ../node/target/debug/glade-node 9099 ../node/target/demo-store
   ```
3. **Install + run the demo**:
   ```
   npm install
   npm run dev
   ```
4. Open the dev URL in **two tabs** (or run a second participant). Edit the
   selection / notes / activity in one — the others converge. Reload a tab and
   the node resyncs it (the node is the backing store). The status dot is
   `live` when connected, `offline` if the node isn't running (local edits
   still work and resync on reconnect).

The wire protocol, folds, and op-hash are the frozen glade contract
(`taut/corpus/glade.*`); the client reproduces them byte-for-byte.
