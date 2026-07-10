// M-LIMP §11 acceptance through the glial path (coverage migrated from the
// binder-era harness with the GC-3 binder deletion) — the whole scenario in one
// scripted harness: two participants converge (lww + log) through the rust
// node, the node restarts and resumes from its store (no data lost), a
// participant writes while offline and reconciles on reconnect, and the echo
// provider answers an EXCHANGE. CHANNEL echo is covered by the rust node tests.
// Participants are browser-shaped: one session + one ws client + one glial
// binder with three mounted bindings; reconnect keeps session + mounts and
// re-ships the session's ops (the node dedups by (origin, seq)).
// Requires the node binary: cargo build --bin glade-node in ../../node.

import test from "node:test";
import assert from "node:assert/strict";
import { rmSync } from "node:fs";
import { join } from "node:path";

import { Session } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import { GlialBinder, MemoryStoreEngine, type Route } from "@owebeeone/glial-runtime";
import {
  ClientBus,
  JSON_PAYLOAD,
  decl,
  here,
  mountView,
  schema,
  startNodeAt,
  stopNode,
  until,
  type MountView,
} from "./helpers.ts";

const SHARE = "app";
const ROUTES: Record<string, Route> = {
  selection: { share: SHARE, gladeId: "app:selection", shape: "value", key: new Uint8Array() },
  notes: { share: SHARE, gladeId: "app:notes", shape: "value", key: new Uint8Array() },
  activity: { share: SHARE, gladeId: "app:activity", shape: "log", key: new Uint8Array() },
};

class Participant {
  readonly origin: string;
  readonly session: Session;
  readonly bus = new ClientBus();
  readonly binder: GlialBinder;
  client!: GladeClient;
  selection: MountView;
  notes: MountView;
  activity: MountView;

  constructor(origin: string) {
    this.origin = origin;
    this.session = new Session(schema, origin);
    this.binder = new GlialBinder(new MemoryStoreEngine(), origin);
    this.selection = mountView(this.binder, this.session, this.bus, decl("app:selection", "value"), { domain: "1" }, ROUTES.selection, JSON_PAYLOAD);
    this.notes = mountView(this.binder, this.session, this.bus, decl("app:notes", "value"), { domain: "1" }, ROUTES.notes, JSON_PAYLOAD);
    this.activity = mountView(this.binder, this.session, this.bus, decl("app:activity", "log"), { domain: "1" }, ROUTES.activity, JSON_PAYLOAD);
  }

  /** Connect (or RE-connect: same session + mounts, a fresh ws client). */
  async connect(url: string): Promise<void> {
    this.client = new GladeClient(schema, this.origin, this.session);
    this.bus.client = this.client;
    this.client.onOps = (ops) => {
      this.session.applyRemote(ops); // truthful heads + own-chain resume
      this.bus.deliver(ops);
    };
    await this.client.connect(url);
    for (const r of Object.values(ROUTES)) await this.client.subscribe(r.share, r.gladeId);
    // resync: re-ship every known op — e.g. writes made while offline reach
    // the node, which dedups by (origin, seq).
    const ops = this.session.dump();
    if (ops.length) this.client.sendOps(ops);
  }
  disconnect(): void {
    this.client.close();
  }
}

test("M-LIMP §11: converge, restart-resume, offline-reconnect, echo", async () => {
  const store = join(here, "..", "..", "node", "target", "mlimp-store");
  rmSync(store, { recursive: true, force: true });
  let node = await startNodeAt(store);

  // --- 1. converge (lww + log) ---
  const A = new Participant("alice");
  const B = new Participant("bob");
  await A.connect(`ws://127.0.0.1:${node.port}`);
  await B.connect(`ws://127.0.0.1:${node.port}`);
  A.selection.write("src/main.rs");
  await until(() => B.selection.value() === "src/main.rs");
  A.activity.write("alice: hello");
  await until(() => B.activity.records().length === 1);

  // --- 2. node restart -> resume from store (no data lost) ---
  A.disconnect();
  B.disconnect();
  await stopNode(node.child);
  node = await startNodeAt(store); // same store dir, new port
  const C = new Participant("carol"); // a fresh joiner sees the persisted state
  await C.connect(`ws://127.0.0.1:${node.port}`);
  await until(() => C.selection.value() === "src/main.rs");
  await until(() => C.activity.records().length === 1);

  // --- 3. offline write -> reconnect -> reconcile ---
  C.disconnect();
  C.selection.write("offline-pick"); // written locally while offline (not yet shipped)
  await C.connect(`ws://127.0.0.1:${node.port}`); // reconnect resyncs the offline op
  const D = new Participant("dave");
  await D.connect(`ws://127.0.0.1:${node.port}`);
  await until(() => D.selection.value() === "offline-pick"); // dave sees C's offline write

  // --- 4. echo provider answers an EXCHANGE ---
  const res = await D.client.exchange(SHARE, "app:echo", utf8("ping"));
  assert.ok(res.ok, "exchange ok");
  assert.equal(new TextDecoder().decode(res.payload!), "ping");

  C.disconnect();
  D.disconnect();
  await stopNode(node.child);
});
