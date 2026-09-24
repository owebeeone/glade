// End-to-end (P2.S4): two TS sessions converge through the real Rust glade
// node over a websocket — the browser-folds half of M-LIMP. Requires the node
// binary built: `cargo build --bin glade-node` in ../../node.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import { GladeClient } from "../src/client.ts";
import { UnresumedChain } from "../src/session.ts";
import type { OpOutcome } from "../src/answers.ts";
import { hex, utf8 } from "../src/bytes.ts";
import { opHash } from "../src/hash.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const bin = join(here, "..", "..", "node", "target", "debug", "glade-node");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

function startNode(): Promise<{ port: number; child: ChildProcess }> {
  const dir = join(here, "..", "..", "node", "target", "it-store");
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
    if (Date.now() - start > ms) throw new Error("timeout waiting for convergence");
    await new Promise((r) => setTimeout(r, 20));
  }
}

test("two TS sessions converge through the rust node over websocket", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    const c1 = new GladeClient(schema, "a");
    const c2 = new GladeClient(schema, "b");
    await c1.connect(url);
    await c2.connect(url);
    await c1.subscribe("sh", "g");
    await c2.subscribe("sh", "g"); // ack ensures c2 is registered before c1 writes

    // c1 writes; c2 receives via the node and folds it
    c1.append("sh", "g", "value", utf8("hello-from-a"));
    await until(() => c2.fold("sh", "g", "value") !== null);
    assert.equal(hex(c2.fold("sh", "g", "value") as Uint8Array), hex(utf8("hello-from-a")));

    // c2 writes back (higher lamport, wins lww); c1 converges to it
    c2.append("sh", "g", "value", utf8("hello-from-b"));
    await until(() => {
      const v = c1.fold("sh", "g", "value");
      return v !== null && hex(v as Uint8Array) === hex(utf8("hello-from-b"));
    });
    assert.equal(
      hex(c1.fold("sh", "g", "value") as Uint8Array),
      hex(c2.fold("sh", "g", "value") as Uint8Array),
    );

    c1.close();
    c2.close();
  } finally {
    child.kill();
  }
});

test("hello binds a principal and resolves on the node's Welcome; plain sessions unchanged", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    // hello(principal) rides the existing wire field and is Welcomed.
    const bound = new GladeClient(schema, "p1");
    await bound.connect(url);
    await bound.hello("alice");
    // a helloed session stays fully usable (subscribe + write + fold).
    await bound.subscribe("sh", "hp");
    bound.append("sh", "hp", "value", utf8("from-alice"));
    assert.equal(hex(bound.fold("sh", "hp", "value") as Uint8Array), hex(utf8("from-alice")));

    // hello() with NO principal is also Welcomed (origin-as-identity).
    const plain = new GladeClient(schema, "p2");
    await plain.connect(url);
    await plain.hello();

    bound.close();
    plain.close();
  } finally {
    child.kill();
  }
});

test("two CRDT writers exchange causal operations through the rust node", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    const alice = new GladeClient(schema, "alice");
    const bob = new GladeClient(schema, "bob");
    await alice.connect(url);
    await bob.connect(url);
    await alice.subscribe("sh", "collaborative-body");
    await bob.subscribe("sh", "collaborative-body");

    const first = alice.append("sh", "collaborative-body", "crdt", utf8("insert-a"));
    assert.deepEqual(first.refs, []);
    await until(() => bob.session.dump().some((op) => op.origin === "alice"));

    const reply = bob.append("sh", "collaborative-body", "crdt", utf8("insert-b"));
    assert.deepEqual(reply.refs.map(({ origin, seq }) => ({ origin, seq })), [{ origin: "alice", seq: 0 }]);
    await until(() => alice.session.dump().some((op) => op.origin === "bob"));
    assert.deepEqual(
      alice.session.dump().map((op) => [op.origin, op.seq, op.shape]).sort(),
      bob.session.dump().map((op) => [op.origin, op.seq, op.shape]).sort(),
    );

    alice.close();
    bob.close();
  } finally {
    child.kill();
  }
});

// Op outcomes (client-writes plan Step 3.3; GladeSubstrateV1 §6, R1 and R7):
// the node answers every op, and the client tells accepted from refused.

test("an accepted append is ok", async () => {
  const { port, child } = await startNode();
  try {
    const c = new GladeClient(schema, "accepted");
    await c.connect(`ws://127.0.0.1:${port}`);
    const outcome = await c.appendOutcome("sh", "g", "value", utf8("one"));
    assert.equal(outcome.ok, true);
    assert.equal(outcome.code, "ok");
    c.close();
  } finally {
    child.kill();
  }
});

test("a repeated append is ok", async () => {
  const { port, child } = await startNode();
  try {
    const c = new GladeClient(schema, "repeated");
    await c.connect(`ws://127.0.0.1:${port}`);
    const first = await c.appendOutcome("sh", "g", "value", utf8("one"));
    const [again] = await c.sendOpsOutcome([first.op]);
    assert.equal(again.ok, true);
    assert.equal(again.code, "ok");
    c.close();
  } finally {
    child.kill();
  }
});

test("a refused append stops its chain", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    // Two clients share an origin, so the second one's first append forks the
    // chain the first one began.
    const first = new GladeClient(schema, "twin");
    const second = new GladeClient(schema, "twin");
    await first.connect(url);
    await second.connect(url);
    assert.equal((await first.appendOutcome("sh", "g", "value", utf8("first"))).ok, true);
    const refused = new Promise<OpOutcome>((resolve) => second.onRefused(resolve));
    second.append("sh", "g", "value", utf8("second"));
    assert.equal((await refused).code, "equivocation");
    // Answer 4: the refused op is dropped, and its chain takes no append ...
    assert.deepEqual(second.session.dump(), []);
    assert.throws(() => second.append("sh", "g", "value", utf8("third")), UnresumedChain);
    // ... until a subscribe of its zone returns, with the replay that brings
    // the node's op 0 (R7).
    await second.subscribe("sh", "g");
    assert.equal(second.session.dump().length, 1);
    const resumed = await second.appendOutcome("sh", "g", "value", utf8("third"));
    assert.equal(resumed.ok, true);
    assert.equal(resumed.op.seq, 1);
    first.close();
    second.close();
  } finally {
    child.kill();
  }
});

// The subscribe outcome (client-writes plan Step 3.4; GladeSubstrateV1 §6,
// R5-R7): a subscribe returns once its replay has arrived.

test("subscribe returns after the replay is folded", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    const writer = new GladeClient(schema, "writer");
    await writer.connect(url);
    for (let i = 0; i < 1999; i++) {
      writer.append("sh", "replay", "log", utf8(`line-${i}`));
    }
    assert.equal((await writer.appendOutcome("sh", "replay", "log", utf8("line-1999"))).ok, true);
    const reader = new GladeClient(schema, "reader");
    await reader.connect(url);
    await reader.subscribe("sh", "replay");
    assert.equal((reader.fold("sh", "replay", "log") as Uint8Array[]).length, 2000);
    writer.close();
    reader.close();
  } finally {
    child.kill();
  }
});

test("subscribeOutcome returns the node's heads, with their hashes", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    const writer = new GladeClient(schema, "writer");
    await writer.connect(url);
    writer.append("sh", "g", "value", utf8("one"));
    const last = await writer.appendOutcome("sh", "g", "value", utf8("two"));
    const reader = new GladeClient(schema, "reader");
    await reader.connect(url);
    const outcome = await reader.subscribeOutcome("sh", "g");
    assert.equal(outcome.ok, true);
    assert.deepEqual(
      outcome.heads.map((h) => [h.origin, h.seq, hex(h.hash ?? new Uint8Array())]),
      [["writer", 1, hex(opHash(schema, last.op as never))]],
    );
    writer.close();
    reader.close();
  } finally {
    child.kill();
  }
});
