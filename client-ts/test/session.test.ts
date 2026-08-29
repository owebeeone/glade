// Session convergence + hydration (P2.S1/S3): two TS sessions exchange ops and
// fold to the same value/log; a dumped store rehydrates to the same result.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import { Session } from "../src/session.ts";
import { UnsupportedShapeError } from "../src/shapes.ts";
import { encodeSwmrAction, SwmrActionError } from "../src/swmr.ts";
import { hex, utf8 } from "../src/bytes.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

/** Bidirectional heads-exchange + gap-ship between two sessions. */
function sync(a: Session, b: Session, share: string): void {
  b.applyRemote(a.missingFor(share, b.heads(share)));
  a.applyRemote(b.missingFor(share, a.heads(share)));
}

test("two sessions converge on an lww value", () => {
  const s1 = new Session(schema, "a");
  const s2 = new Session(schema, "b");
  s1.append("sh", "g", "value", utf8("from-a"));
  s2.append("sh", "g", "value", utf8("from-b")); // concurrent write
  sync(s1, s2, "sh");
  const v1 = s1.fold("sh", "g", "value") as Uint8Array;
  const v2 = s2.fold("sh", "g", "value") as Uint8Array;
  assert.equal(hex(v1), hex(v2)); // converged (lamport tie -> origin "b" wins)
});

test("two sessions converge on a log's order", () => {
  const s1 = new Session(schema, "a");
  const s2 = new Session(schema, "b");
  s1.append("sh", "feed", "log", utf8("a-1"));
  s2.append("sh", "feed", "log", utf8("b-1"));
  s1.append("sh", "feed", "log", utf8("a-2"));
  sync(s1, s2, "sh");
  const l1 = (s1.fold("sh", "feed", "log") as Uint8Array[]).map(hex);
  const l2 = (s2.fold("sh", "feed", "log") as Uint8Array[]).map(hex);
  assert.deepEqual(l1, l2); // identical deterministic order both sides
  assert.equal(l1.length, 3);
});

test("local appends form a valid chain (no break on re-store)", () => {
  const s = new Session(schema, "a");
  s.append("sh", "g", "value", utf8("one"));
  s.append("sh", "g", "value", utf8("two"));
  s.append("sh", "g", "value", utf8("three"));
  // restore re-runs append (re-validates the prev chain); must not throw
  const restored = Session.restore(schema, "a", s.dump());
  assert.equal(
    hex(restored.fold("sh", "g", "value") as Uint8Array),
    hex(s.fold("sh", "g", "value") as Uint8Array),
  );
});

test("offline writes survive hydration and reconcile on reconnect", () => {
  // session writes offline, is dumped (persisted), restored, then syncs.
  const offline = new Session(schema, "a");
  offline.append("sh", "g", "value", utf8("offline-edit"));
  const restored = Session.restore(schema, "a", offline.dump());

  const peer = new Session(schema, "b");
  peer.append("sh", "g", "value", utf8("peer-edit"));
  sync(restored, peer, "sh");
  assert.equal(
    hex(restored.fold("sh", "g", "value") as Uint8Array),
    hex(peer.fold("sh", "g", "value") as Uint8Array),
  );
});

for (const shape of ["message", "stream", "exchange", "window"] as const) {
  test(`unsupported ${shape} fails before session mutation`, () => {
    const s = new Session(schema, "a");
    assert.throws(() => s.append("sh", "g", shape, utf8("bad")), (error) => {
      assert(error instanceof UnsupportedShapeError);
      assert.equal(error.code, "GLADE_UNSUPPORTED_SHAPE");
      assert.equal(error.shape, shape);
      assert.equal(error.operation, "append");
      assert.match(error.message, /value, log, swmr/);
      return true;
    });
    assert.deepEqual(s.dump(), []);

    // The failed append did not consume a sequence or lamport number.
    const first = s.append("sh", "g", "value", utf8("ok"));
    assert.equal(first.seq, 0);
    assert.equal(first.lamport, 1);
  });
}

test("crdt appends retain causal stream heads without becoming a value/log fold", () => {
  const alice = new Session(schema, "alice");
  const bob = new Session(schema, "bob");

  const a0 = alice.append("sh", "doc.body", "crdt", utf8("A"));
  assert.deepEqual(a0.refs, []);
  bob.applyRemote([a0]);

  const b0 = bob.append("sh", "doc.body", "crdt", utf8("B"));
  assert.deepEqual(b0.refs.map(({ origin, seq }) => ({ origin, seq })), [
    { origin: "alice", seq: 0 },
  ]);
  alice.applyRemote([b0]);

  const a1 = alice.append("sh", "doc.body", "crdt", utf8("C"));
  assert.deepEqual(a1.refs.map(({ origin, seq }) => ({ origin, seq })), [
    { origin: "alice", seq: 0 },
    { origin: "bob", seq: 0 },
  ]);
  assert.throws(() => alice.fold("sh", "doc.body", "crdt"), UnsupportedShapeError);
  assert.equal(Session.restore(schema, "alice", alice.dump()).dump().length, 3);
});

test("unsupported fold does not fall through to value", () => {
  const s = new Session(schema, "a");
  s.append("sh", "g", "value", utf8("kept"));
  assert.throws(() => s.fold("sh", "g", "atom"), UnsupportedShapeError);
  assert.equal(s.dump().length, 1);
});

test("an unsupported remote batch is rejected atomically", () => {
  const peer = new Session(schema, "peer");
  const good = peer.append("sh", "g", "value", utf8("good"));
  const bad = { ...good, origin: "legacy", shape: "message" };
  const target = new Session(schema, "target");

  assert.throws(() => target.applyRemote([good, bad]), UnsupportedShapeError);
  assert.deepEqual(target.dump(), []);
  assert.throws(() => Session.restore(schema, "target", [bad]), UnsupportedShapeError);
});

test("swmr actions append, replicate, and restore without becoming a generic fold", () => {
  const source = new Session(schema, "writer-a");
  source.append("sh", "ws.files", "swmr", encodeSwmrAction("snapshot", utf8("whole-0")));
  source.append("sh", "ws.files", "swmr", encodeSwmrAction("delta", utf8("whole-1")));

  const target = new Session(schema, "reader");
  target.applyRemote(source.dump());
  assert.equal(target.dump().length, 2);
  assert.equal(Session.restore(schema, "reader", target.dump()).dump().length, 2);
  assert.throws(() => target.fold("sh", "ws.files", "swmr"), UnsupportedShapeError);
});

test("malformed swmr actions fail before local or remote session mutation", () => {
  const local = new Session(schema, "writer-a");
  assert.throws(
    () => local.append("sh", "ws.files", "swmr", new Uint8Array([1, 99])),
    SwmrActionError,
  );
  assert.deepEqual(local.dump(), []);

  const good = new Session(schema, "writer-a").append(
    "sh",
    "ws.files",
    "swmr",
    encodeSwmrAction("snapshot", utf8("whole")),
  );
  const bad = { ...good, origin: "legacy", payload: new Uint8Array([2, 0]) };
  const target = new Session(schema, "reader");
  assert.throws(() => target.applyRemote([good, bad]), SwmrActionError);
  assert.deepEqual(target.dump(), []);
});
