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
import { hex, utf8 } from "../src/bytes.ts";

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
