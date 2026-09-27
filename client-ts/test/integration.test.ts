// End-to-end (P2.S4): two TS sessions converge through the real Rust glade
// node over a websocket — the browser-folds half of M-LIMP. Requires the node
// binary built: `cargo build --bin glade-node` in ../../node.

import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import { GladeClient } from "../src/client.ts";
import { UnresumedChain } from "../src/session.ts";
import type { OpOutcome, ZoneRefusal } from "../src/answers.ts";
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

// A zone refused after its ack (follow-up F13; the owner's answer (a) to F5's
// question): two booted nodes on loopback, relays off (`--peer`), each under
// GLADE_HOME and HOME in a fresh temporary directory, never `~/.glade`.

/** `glade-node endpoint-id --name <name>`, as an operator reads it: the
 *  instance's endpoint id, its key minted first. */
function endpointId(home: string, name: string): string {
  const env = { GLADE_HOME: home, HOME: home };
  return execFileSync(bin, ["endpoint-id", "--name", name], { env, timeout: 15_000, encoding: "utf8" }).trim();
}

/** The instance `name` booted with `args` before its port, 0: the node, the
 *  lines it printed before `listening <port>`, and the port. */
function bootNode(home: string, name: string, args: string[]): Promise<{ child: ChildProcess; lines: string[]; port: number }> {
  const env = { GLADE_HOME: home, HOME: home };
  const child = spawn(bin, ["--profile", "local", "--name", name, ...args, "0"], { env, stdio: ["ignore", "pipe", "ignore"] });
  const lines: string[] = [];
  let rest = "";
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(`node ${name} start timeout, after ${JSON.stringify(lines)}`)), 15_000);
    child.on("exit", (code) => {
      clearTimeout(t);
      reject(new Error(`node ${name} exited ${code}, after ${JSON.stringify(lines)}`));
    });
    child.stdout!.on("data", (d: Buffer) => {
      const parts = (rest + d.toString()).split("\n");
      rest = parts.pop() ?? "";
      for (const line of parts) {
        const m = /^listening (\d+)$/.exec(line);
        if (m) {
          clearTimeout(t);
          resolve({ child, lines: [...lines], port: Number(m[1]) });
        }
        lines.push(line);
      }
    });
  });
}

/** The rest of the first of `lines` that starts `word `. */
function said(lines: string[], word: string): string {
  const found = lines.find((line) => line.startsWith(`${word} `));
  assert.ok(found, `no \`${word}\` line in ${JSON.stringify(lines)}`);
  return found.slice(word.length + 1);
}

test("a zone refused after its ack reaches onZoneRefused, and is no longer live", async () => {
  // B loads grazel-app.glade, so it serves ws-razel, and admits A's endpoint
  // but grants A's node nothing; A dials B. A acks a subscribe to
  // ws-razel/ws.tree from its empty replica, and B's refusal of the forwarded
  // read then reaches the client as an Error naming the zone and no op (F5).
  const home = mkdtempSync(join(tmpdir(), "glade-client-ts-f13-"));
  const nodes: ChildProcess[] = [];
  try {
    const [aId, bId] = [endpointId(home, "a"), endpointId(home, "b")];
    const app = join(here, "..", "..", "apps", "grazel-app.glade");
    const b = await bootNode(home, "b", ["--app", app, "--peer", aId]);
    nodes.push(b.child);
    const bNode = said(b.lines, "node");
    // `peer <tag> <ip:port>`: where B's endpoint listens.
    const bAt = said(b.lines, "peer").split(" ")[1];
    const a = await bootNode(home, "a", ["--peer", `${bId}@${bAt}`]);
    nodes.push(a.child);
    const linked = a.lines.some((line) => line.startsWith(`home round with node ${bNode}`));
    assert.ok(linked, `A took no home round with B: ${JSON.stringify(a.lines)}`);

    const client = new GladeClient(schema, "reader");
    await client.connect(`ws://127.0.0.1:${a.port}`);
    const refusals: ZoneRefusal[] = [];
    client.onZoneRefused((r) => refusals.push(r));
    const acked = await client.subscribeOutcome("ws-razel", "ws.tree");
    assert.deepEqual(acked, { ok: true, heads: [], code: "ok", message: "" }, "A acks from its empty replica");
    await until(() => refusals.length > 0, 10_000);
    const [refused] = refusals;
    const expected = { share: "ws-razel", glade_id: "ws.tree", key: new Uint8Array(), code: "unauthorized", message: refused.message };
    assert.deepEqual(refused, expected);
    assert.ok(refused.message.startsWith(`refused by node ${bNode}, which serves ws-razel: `), refused.message);
    assert.equal(client.live("ws-razel", "ws.tree"), false);

    // The refusal is not kept: a later subscribe asks A again, which acks it,
    // forwards the read again, and relays B's refusal again.
    assert.equal((await client.subscribeOutcome("ws-razel", "ws.tree")).ok, true);
    await until(() => refusals.length > 1, 10_000);
    assert.deepEqual(refusals[1], refused);
    assert.equal(client.live("ws-razel", "ws.tree"), false);
    client.close();
  } finally {
    for (const node of nodes) {
      node.kill();
    }
    rmSync(home, { recursive: true, force: true });
  }
});
