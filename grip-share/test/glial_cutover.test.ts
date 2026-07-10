// GC-3 per-binding cutover interop (Lane T step 3b) — proves each demo binding
// is WIRE-BYTE compatible across the cutover: a binder-era participant (the old
// grip-share tap↔session coupling) and a glial-era participant (GlialBinder +
// SessionDestination over the same WS carrier) converge BOTH directions through
// the real rust node, and the ops a glial participant ships are byte-identical
// in (share, key, shape, payload) to the binder era. One test per cut binding.
// Requires the node binary: cargo build --bin glade-node in ../../node.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import { encode as tautEncode, decode as tautDecode } from "../../client-ts/src/taut/codec.ts";
import { Session, type Op } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import { GripShareBinder, type GrokLike, type PayloadCodec, type Scope, type SharableTap } from "../src/binder.ts";
import {
  GlialBinder,
  MemoryStoreEngine,
  SessionDestination,
  type Fill,
  type InstanceEvent,
  type OpBus,
  type Route,
  type SessionLike,
  type WireOp,
} from "@owebeeone/glial-runtime";
import type { BindingDecl } from "@owebeeone/glade-decl";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const bin = join(here, "..", "..", "node", "target", "debug", "glade-node");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

// ---- shared harness ---------------------------------------------------------

function startNode(tag: string): Promise<{ port: number; child: ChildProcess }> {
  const dir = join(here, "..", "..", "node", "target", `it-cutover-${tag}`);
  rmSync(dir, { recursive: true, force: true });
  const child = spawn(bin, ["0", dir], { stdio: ["ignore", "pipe", "inherit"] });
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("node start timeout")), 8000);
    child.stdout!.on("data", (d: Buffer) => {
      const m = /listening (\d+)/.exec(d.toString());
      if (m) {
        clearTimeout(t);
        resolve({ port: Number(m[1]), child });
      }
    });
  });
}
async function until(pred: () => boolean, ms = 3000): Promise<void> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > ms) throw new Error("timeout");
    await new Promise((r) => setTimeout(r, 20));
  }
}
const dec = new TextDecoder();
const jsonBytes = (v: unknown) => utf8(JSON.stringify(v ?? null));

/** The WS carrier as glial's OpBus (the production wiring, cf. demo glial.ts):
 *  publish ships to the node; inbound node ops fan to every SessionDestination.
 *  Also records everything published — the wire-byte evidence. */
class ClientBus implements OpBus {
  published: WireOp[] = [];
  private handlers = new Set<(ops: WireOp[]) => void>();
  constructor(private readonly client: GladeClient) {}
  publish(ops: WireOp[]): void {
    this.published.push(...ops);
    this.client.sendOps(ops as unknown as Op[]);
  }
  onOps(handler: (ops: WireOp[]) => void): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }
  deliver(ops: Op[]): void {
    for (const h of [...this.handlers]) h(ops as unknown as WireOp[]);
  }
}

function decl(id: string, shape: "value" | "log", domain: "account" | "document", zone: "commons" | "private"): BindingDecl {
  return {
    glade_id: { id },
    shape,
    authority: "share",
    source: null,
    domain,
    zone,
    retention: { policy: "from_cursor", ttl_ms: null },
  };
}

/** A glial-era participant: one mounted binding over the real node. `codec`
 *  defaults to the JSON default codec; a typed surface passes its own. */
async function glialParticipant(
  origin: string,
  url: string,
  d: BindingDecl,
  fill: Fill,
  route: Route,
  codec: { encode(v: unknown): Uint8Array; decode(b: Uint8Array): unknown } = {
    encode: jsonBytes,
    decode: (b) => JSON.parse(dec.decode(b)),
  },
) {
  const session = new Session(schema, origin);
  const client = new GladeClient(schema, origin, session);
  const bus = new ClientBus(client);
  // the demo's carrier wiring (glade.ts): the session sees EVERY inbound op —
  // truthful heads AND own-chain resume for a fresh page session (own-origin
  // ops are echo-guarded out of the instance path; the session store dedups).
  client.onOps = (ops) => {
    session.applyRemote(ops);
    bus.deliver(ops);
  };
  // mount BEFORE subscribe (the demo's order: registerAllTaps then
  // startGladeSync) so the replay the node sends on subscribe is not dropped.
  const glial = new GlialBinder(new MemoryStoreEngine(), origin);
  const events: InstanceEvent[] = [];
  const mount = glial.mount(d, fill, (e) => events.push(e), {
    glade: new SessionDestination(session as unknown as SessionLike, bus, route),
  });
  await client.connect(url);
  await client.subscribe(route.share, route.gladeId, route.key.length ? route.key : undefined);
  return {
    client,
    bus,
    events,
    mount,
    session,
    /** decoded view of a value surface */
    value: () => {
      const e = events[events.length - 1];
      return e?.value ? codec.decode(e.value) : undefined;
    },
    /** decoded ordered view of a log surface */
    records: () => {
      const e = events[events.length - 1];
      return (e?.records ?? []).map((r) => codec.decode(r.payload));
    },
    setJson: (v: unknown) => mount.instance.write(jsonBytes(v)),
    append: (v: unknown) => mount.instance.write(codec.encode(v)),
  };
}

/** A binder-era participant: the old grip-share coupling, one fake share tap. */
function fakeAtom(gladeId: string, shape = "value") {
  let v: unknown = "";
  const ls = new Set<() => void>();
  return {
    share: { gladeId, shape },
    get: () => v,
    set: (x: unknown) => {
      if (x !== v) {
        v = x;
        ls.forEach((l) => l());
      }
    },
    getShareValue: () => v,
    applyShareValue: (x: unknown) => {
      if (x !== v) {
        v = x;
        ls.forEach((l) => l());
      }
    },
    subscribeShare: (l: () => void) => {
      ls.add(l);
      return () => ls.delete(l);
    },
  };
}
async function binderParticipant(origin: string, url: string, tap: SharableTap, scope: Scope, codecs?: Map<string, PayloadCodec>) {
  const session = new Session(schema, origin);
  const grok: GrokLike = { listSharedTaps: () => [tap] };
  const binder = new GripShareBinder(grok, session, codecs, scope);
  const client = new GladeClient(schema, origin, session);
  const shipped: Op[] = [];
  client.onOps = (ops) => binder.applyRemote(ops);
  binder.onLocalOps = (ops) => {
    shipped.push(...ops);
    client.sendOps(ops);
  };
  await client.connect(url);
  binder.bind();
  for (const s of binder.subscriptions()) await client.subscribe(s.share, s.gladeId, s.key.length ? s.key : undefined);
  return { binder, client, shipped };
}

// ---- binding 1: app:status (value, account domain, commons) -----------------

test("cutover 1/4 app:status: binder-era <-> glial-era converge; ops byte-identical", async () => {
  const { port, child } = await startNode("status");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const user = "u1";
    const share = `account:${user}`; // the demo manifest's account domain, self-filled
    const scope: Scope = { resolve: () => ({ share, key: new Uint8Array() }) };
    const route: Route = { share, gladeId: "app:status", shape: "value", key: new Uint8Array() };

    // two tabs of the SAME user — one old-era, one new-era.
    const statusTap = fakeAtom("app:status");
    const A = await binderParticipant("tab-a", url, statusTap, scope);
    const B = await glialParticipant("tab-b", url, decl("app:status", "value", "account", "commons"), { domain: user }, route);

    // old -> new
    statusTap.set("busy");
    await until(() => B.value() === "busy");

    // new -> old
    B.setJson("away");
    await until(() => statusTap.get() === "away");

    // wire-byte evidence: the glial-era op is byte/field-identical to the
    // binder era — same share, commons key, value shape, JSON payload bytes.
    const gOp = B.bus.published[0];
    const bOp = A.shipped[0];
    assert.equal(gOp.share, bOp.share);
    assert.equal(gOp.glade_id, "app:status");
    assert.equal((gOp as unknown as Op).shape, bOp.shape);
    assert.deepEqual([...gOp.key], [...bOp.key]);
    assert.deepEqual([...gOp.payload], [...jsonBytes("away")]);
    assert.deepEqual([...bOp.payload], [...jsonBytes("busy")]);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

// ---- binding 2: app:notes (value, doc domain, commons) -----------------------

test("cutover 2/4 app:notes: late glial joiner hydrates binder-era state; converge both ways", async () => {
  const { port, child } = await startNode("notes");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1"; // the demo manifest's doc domain, doc=1
    const scope: Scope = { resolve: () => ({ share, key: new Uint8Array() }) };
    const route: Route = { share, gladeId: "app:notes", shape: "value", key: new Uint8Array() };

    // binder-era participant writes BEFORE the glial one exists — the store
    // holds binder-era bytes the glial era must fold (existing-store compat).
    const notesTap = fakeAtom("app:notes");
    const A = await binderParticipant("a", url, notesTap, scope);
    notesTap.set("agenda v1");
    await new Promise((r) => setTimeout(r, 100));

    // late glial joiner: subscribe replays the stored (binder-era) op.
    const B = await glialParticipant("b", url, decl("app:notes", "value", "document", "commons"), { domain: "1" }, route);
    await until(() => B.value() === "agenda v1");

    // new -> old and old -> new still converge live.
    B.setJson("agenda v2");
    await until(() => notesTap.get() === "agenda v2");
    notesTap.set("agenda v3");
    await until(() => B.value() === "agenda v3");

    // wire-byte evidence: glial op == binder-era encoding.
    const gOp = B.bus.published[0];
    assert.equal(gOp.share, share);
    assert.equal(gOp.glade_id, "app:notes");
    assert.deepEqual([...gOp.payload], [...jsonBytes("agenda v2")]);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

// ---- binding 3: app:selection (value, doc domain, PRIVATE zone) --------------

test("cutover 3/4 app:selection: private zone stays per-user across eras; key bytes identical", async () => {
  const { port, child } = await startNode("selection");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const keyFor = (u: string) => utf8(`self:${u}`); // the demo manifest's private zone
    // alice stays binder-era; bob is cut over to glial.
    const aliceScope: Scope = { resolve: () => ({ share, key: keyFor("alice") }) };
    const bobRoute: Route = { share, gladeId: "app:selection", shape: "value", key: keyFor("bob") };

    const aliceSel = fakeAtom("app:selection");
    const alice = await binderParticipant("alice", url, aliceSel, aliceScope);
    const bob = await glialParticipant(
      "bob",
      url,
      decl("app:selection", "value", "document", "private"),
      { domain: "1", zone: "private", key: "bob" },
      bobRoute,
    );

    aliceSel.set("src/main.rs");
    bob.setJson("Cargo.toml");
    await until(() => bob.value() === "Cargo.toml");
    await until(() => aliceSel.get() === "src/main.rs");
    // let any (erroneous) cross-delivery arrive, then assert isolation held.
    await new Promise((r) => setTimeout(r, 120));
    assert.equal(aliceSel.get(), "src/main.rs"); // never bob's pick
    assert.equal(bob.value(), "Cargo.toml"); // never alice's pick

    // wire-byte evidence: the glial op carries the same self-keyed zone key
    // format and JSON payload the binder era wrote.
    const gOp = bob.bus.published[0];
    const bOp = alice.shipped[0];
    assert.equal(gOp.share, bOp.share);
    assert.deepEqual([...gOp.key], [...keyFor("bob")]);
    assert.deepEqual([...bOp.key], [...keyFor("alice")]);
    assert.deepEqual([...gOp.payload], [...jsonBytes("Cargo.toml")]);

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

test("cutover 4/4 app:activity: typed log interleaves across eras; taut payload bytes identical", async () => {
  const { port, child } = await startNode("activity");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const scope: Scope = { resolve: () => ({ share, key: new Uint8Array() }) };
    const route: Route = { share, gladeId: "app:activity", shape: "log", key: new Uint8Array() };

    // binder-era participant: log tap + the ChatLine codec (the demo's old wiring).
    let list: unknown[] = [];
    const logTap: SharableTap = {
      share: { gladeId: "app:activity", shape: "log" },
      applyShareValue: (v: unknown) => (list = v as unknown[]),
    };
    const A = await binderParticipant("a", url, logTap, scope, new Map([["app:activity", chatCodec]]));
    const B = await glialParticipant(
      "b",
      url,
      decl("app:activity", "log", "document", "commons"),
      { domain: "1" },
      route,
      chatCodec,
    );

    const l1: ChatLine = { ts: 1000, user: "alice", text: "opened src/main.rs" };
    const l2: ChatLine = { ts: 2000, user: "bob", text: "posted from glial" };
    const l3: ChatLine = { ts: 3000, user: "alice", text: "replied" };

    A.binder.appendLog("app:activity", l1);
    await until(() => B.records().length === 1);
    B.append(l2);
    await until(() => list.length === 2);
    A.binder.appendLog("app:activity", l3);
    await until(() => B.records().length === 3);

    // both eras converge to the SAME ordered, decoded list.
    assert.deepEqual(B.records(), list);
    assert.deepEqual(
      (list as ChatLine[]).map((l) => l.text),
      ["opened src/main.rs", "posted from glial", "replied"],
    );

    // wire-byte evidence: the glial-era entry is the taut ChatLine encoding,
    // byte-identical to what the binder era ships for the same entry.
    const gOp = B.bus.published[0];
    assert.equal(gOp.glade_id, "app:activity");
    assert.deepEqual([...gOp.payload], [...chatCodec.encode(l2)]);
    const bOp = A.shipped[0];
    assert.deepEqual([...bOp.payload], [...chatCodec.encode(l1)]);

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
  const { port, child } = await startNode("reload");
  const url = `ws://127.0.0.1:${port}`;
  try {
    const share = "doc:1";
    const route: Route = { share, gladeId: "app:notes", shape: "value", key: new Uint8Array() };
    const d = decl("app:notes", "value", "document", "commons");

    // page life 1: write, then the tab goes away.
    const P1 = await glialParticipant("tab-x", url, d, { domain: "1" }, route);
    P1.setJson("first note");
    await new Promise((r) => setTimeout(r, 100));
    P1.client.close();

    // page life 2: SAME origin, fresh session/binder — the reload.
    const P2 = await glialParticipant("tab-x", url, d, { domain: "1" }, route);
    await until(() => P2.session.dump().length >= 1); // replay hydrated the session
    P2.setJson("second note");

    // the op continued the chain (seq 1, not a forked seq 0)...
    assert.equal(P2.bus.published[0].seq, 1);
    // ...so the node accepts it and a witness converges to the NEW value.
    const W = await glialParticipant("witness", url, d, { domain: "1" }, route);
    await until(() => W.value() === "second note");

    P2.client.close();
    W.client.close();
  } finally {
    child.kill();
  }
});
