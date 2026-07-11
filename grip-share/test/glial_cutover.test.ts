// GC-3 per-binding cutover — the per-binding wire-format pins (Lane T 3b).
// During the cutover each of these ran CROSS-ERA (a grip-share binder
// participant against a glial participant; see git history fc0f7cf..5da0533)
// proving byte-identical convergence through the real rust node. The binder is
// deleted now, so both participants are glial mounts and the wire format is
// pinned ABSOLUTELY instead: (share, key, shape) per the demo manifest and
// payload bytes equal to the era-invariant encodings (JSON / taut ChatLine).
// Requires the node binary: cargo build --bin glade-node in ../../node.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import { encode as tautEncode, decode as tautDecode } from "../../client-ts/src/taut/codec.ts";
import { type Op } from "../../client-ts/src/session.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import { feedSession, GlialBinder, IndexedDbStoreEngine, type Route, type SessionLike } from "@owebeeone/glial-runtime";
import { IDBFactory } from "fake-indexeddb";
import { Session } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { ClientBus, decl, glialParticipant, here, jsonBytes, JSON_PAYLOAD, mountView, schema, startNode, until } from "./helpers.ts";

// ---- binding 1: app:status (value, account domain, commons) -----------------

test("cutover 1/4 app:status: two tabs of one user converge; op fields/bytes pinned", async () => {
  const { port, child } = await startNode("cutover-status");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "account:u1"; // the demo manifest's account domain, self-filled
    const route: Route = { share, gladeId: "app:status", shape: "value", key: new Uint8Array() };
    const d = decl("app:status", "value", "account");

    const A = await glialParticipant("tab-a", url, d, { domain: "u1" }, route);
    const B = await glialParticipant("tab-b", url, d, { domain: "u1" }, route);

    A.write("busy");
    await until(() => B.value() === "busy");
    B.write("away");
    await until(() => A.value() === "away");

    // wire pins: the op is addressed and encoded exactly as the binder era.
    const op = B.bus.published[0] as unknown as Op;
    assert.equal(op.share, "account:u1");
    assert.equal(op.glade_id, "app:status");
    assert.equal(op.shape, "value");
    assert.equal(op.key.length, 0); // commons
    assert.deepEqual([...op.payload], [...jsonBytes("away")]);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

// ---- binding 2: app:notes (value, doc domain, commons) -----------------------

test("cutover 2/4 app:notes: late joiner hydrates stored bytes; converge both ways", async () => {
  const { port, child } = await startNode("cutover-notes");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const route: Route = { share, gladeId: "app:notes", shape: "value", key: new Uint8Array() };
    const d = decl("app:notes", "value");

    // A writes BEFORE B exists — B must fold the STORED bytes off the replay.
    const A = await glialParticipant("a", url, d, { domain: "1" }, route);
    A.write("agenda v1");
    await new Promise((r) => setTimeout(r, 100));

    const B = await glialParticipant("b", url, d, { domain: "1" }, route);
    await until(() => B.value() === "agenda v1");

    B.write("agenda v2");
    await until(() => A.value() === "agenda v2");
    A.write("agenda v3");
    await until(() => B.value() === "agenda v3");

    const op = B.bus.published[0] as unknown as Op;
    assert.equal(op.share, "doc:1");
    assert.equal(op.glade_id, "app:notes");
    assert.deepEqual([...op.payload], [...jsonBytes("agenda v2")]);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

// ---- binding 3: app:selection (value, doc domain, PRIVATE zone) --------------

test("cutover 3/4 app:selection: private zone stays per-user; key bytes pinned", async () => {
  const { port, child } = await startNode("cutover-selection");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const keyFor = (u: string) => utf8(`self:${u}`); // the demo manifest's private zone
    const d = decl("app:selection", "value", "document", "private");
    const routeFor = (u: string): Route => ({ share, gladeId: "app:selection", shape: "value", key: keyFor(u) });

    const alice = await glialParticipant("alice", url, d, { domain: "1", zone: "private", key: "alice" }, routeFor("alice"));
    const bob = await glialParticipant("bob", url, d, { domain: "1", zone: "private", key: "bob" }, routeFor("bob"));

    alice.write("src/main.rs");
    bob.write("Cargo.toml");
    await until(() => bob.value() === "Cargo.toml");
    await until(() => alice.value() === "src/main.rs");
    // let any (erroneous) cross-delivery arrive, then assert isolation held.
    await new Promise((r) => setTimeout(r, 120));
    assert.equal(alice.value(), "src/main.rs"); // never bob's pick
    assert.equal(bob.value(), "Cargo.toml"); // never alice's pick

    const gOp = bob.bus.published[0] as unknown as Op;
    assert.deepEqual([...gOp.key], [...keyFor("bob")]);
    assert.deepEqual([...gOp.payload], [...jsonBytes("Cargo.toml")]);
    const aOp = alice.bus.published[0] as unknown as Op;
    assert.deepEqual([...aOp.key], [...keyFor("alice")]);

    alice.client.close();
    bob.client.close();
  } finally {
    child.kill();
  }
});

// ---- binding 4: app:activity (log, doc domain, commons, taut ChatLine) ------

const appSchema = loadSchema(
  JSON.parse(readFileSync(join(here, "..", "..", "demo", "ir", "workspace.ir.json"), "utf8")),
);
interface ChatLine {
  ts: number;
  user: string;
  text: string;
}
const chatCodec = {
  encode: (v: unknown) => tautEncode(appSchema, "ChatLine", v as never),
  decode: (b: Uint8Array) => tautDecode(appSchema, "ChatLine", b),
};

test("cutover 4/4 app:activity: typed log interleaves; taut payload bytes pinned", async () => {
  const { port, child } = await startNode("cutover-activity");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const route: Route = { share, gladeId: "app:activity", shape: "log", key: new Uint8Array() };
    const d = decl("app:activity", "log");

    const A = await glialParticipant("a", url, d, { domain: "1" }, route, chatCodec);
    const B = await glialParticipant("b", url, d, { domain: "1" }, route, chatCodec);

    const l1: ChatLine = { ts: 1000, user: "alice", text: "opened src/main.rs" };
    const l2: ChatLine = { ts: 2000, user: "bob", text: "posted from glial" };
    const l3: ChatLine = { ts: 3000, user: "alice", text: "replied" };

    A.write(l1);
    await until(() => B.records().length === 1);
    B.write(l2);
    await until(() => A.records().length === 2);
    A.write(l3);
    await until(() => B.records().length === 3);

    // both participants converge to the SAME ordered, decoded list.
    assert.deepEqual(B.records(), A.records());
    assert.deepEqual(
      (A.records() as ChatLine[]).map((l) => l.text),
      ["opened src/main.rs", "posted from glial", "replied"],
    );

    // wire pin: each entry is the taut ChatLine encoding, byte-for-byte.
    const gOp = B.bus.published[0] as unknown as Op;
    assert.equal(gOp.glade_id, "app:activity");
    assert.equal(gOp.shape, "log");
    assert.deepEqual([...gOp.payload], [...chatCodec.encode(l2)]);
    assert.deepEqual([...(A.bus.published[0] as unknown as Op).payload], [...chatCodec.encode(l1)]);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

// ---- regression: reload-resume (same origin, fresh session) -----------------
// A tab reload keeps its stable origin but rebuilds session + binder. The
// session must resume its own chain off the node replay (the demo carrier
// feeds every inbound op to the session) — otherwise the next write restarts
// at seq 0: a forked chain the node rightly drops (observed live, 2026-07-10).

test("cutover regression: a reloaded tab (same origin, fresh session) resumes its chain", async () => {
  const { port, child } = await startNode("cutover-reload");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const route: Route = { share, gladeId: "app:notes", shape: "value", key: new Uint8Array() };
    const d = decl("app:notes", "value");

    // page life 1: write, then the tab goes away.
    const P1 = await glialParticipant("tab-x", url, d, { domain: "1" }, route);
    P1.write("first note");
    await new Promise((r) => setTimeout(r, 100));
    P1.client.close();

    // page life 2: SAME origin, fresh session/binder — the reload.
    const P2 = await glialParticipant("tab-x", url, d, { domain: "1" }, route);
    await until(() => P2.session.dump().length >= 1); // replay hydrated the session
    P2.write("second note");

    // the op continued the chain (seq 1, not a forked seq 0)...
    assert.equal((P2.bus.published[0] as unknown as Op).seq, 1);
    // ...so the node accepts it and a witness converges to the NEW value.
    const W = await glialParticipant("witness", url, d, { domain: "1" }, route);
    await until(() => W.value() === "second note");

    P2.client.close();
    W.client.close();
  } finally {
    child.kill();
  }
});

test("cutover regression: offline-from-boot after a reload does not fork (persisted chain hydrates the session)", async () => {
  const { port, child } = await startNode("cutover-offline");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const route: Route = { share, gladeId: "app:notes", shape: "value", key: new Uint8Array() };
    const d = decl("app:notes", "value");
    const fill = { domain: "1" };
    const idb = new IDBFactory(); // one fake browser profile across page lives

    // page life 1: connected, persistent engine (the demo's GC-4 slot), write.
    const P1 = await glialParticipant(
      "tab-off", url, d, fill, route, JSON_PAYLOAD,
      await IndexedDbStoreEngine.open("glial:tab-off", idb),
    );
    P1.write("first note");
    await new Promise((r) => setTimeout(r, 100));
    await (P1.engine as IndexedDbStoreEngine).flush();
    P1.client.close();

    // page life 2: same origin + same IndexedDB, fresh session — and the node
    // is UNREACHABLE at boot: no replay exists to hydrate the chain.
    const engine2 = await IndexedDbStoreEngine.open("glial:tab-off", idb);
    const session2 = new Session(schema, "tab-off");
    const client2 = new GladeClient(schema, "tab-off", session2);
    const bus2 = new ClientBus(); // client attached only once "online" below
    client2.onOps = (ops) => bus2.deliver(ops);
    feedSession(session2 as unknown as SessionLike, bus2);
    const binder2 = new GlialBinder(engine2, "tab-off");
    const view2 = mountView(binder2, session2, bus2, d, fill, route, JSON_PAYLOAD);

    // own state is visible from IndexedDB alone (no node)...
    assert.equal(view2.value(), "first note");
    // ...and the OFFLINE write continues the persisted chain, not a fork at 0
    // (attachGlade hydrated the fresh session from the wholesale records).
    view2.write("second note");
    assert.equal((bus2.published[0] as unknown as Op).seq, 1);

    // the tab comes online later: connect, subscribe, re-ship (demo boot path).
    bus2.client = client2;
    await client2.connect(url);
    await client2.subscribe(route.share, route.gladeId, undefined);
    client2.sendOps(session2.dump() as Op[]);

    // the node ACCEPTED the offline write — a witness converges to it (a
    // forked chain would have been dropped and left "first note").
    const W = await glialParticipant("witness-off", url, d, fill, route);
    await until(() => W.value() === "second note");

    client2.close();
    W.client.close();
  } finally {
    child.kill();
  }
});
