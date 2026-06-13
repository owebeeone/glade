// Session convergence + hydration (P2.S1/S3): two TS sessions exchange ops and
// fold to the same value/log; a dumped store rehydrates to the same result.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import { Session } from "../src/session.ts";
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
