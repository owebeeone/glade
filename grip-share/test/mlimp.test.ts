// M-LIMP §11 acceptance — the whole scenario in one scripted harness:
//   two participants converge (lww + log) through the rust node, the node
//   restarts and resumes from its store (no data lost), a participant writes
//   while offline and reconciles on reconnect, and the echo provider answers an
//   EXCHANGE. CHANNEL echo is covered by the rust node tests (echo.rs + server).
// Requires the node binary: cargo build --bin glade-node in ../../node.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import { Session } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import { GripShareBinder, SHARE, type GrokLike, type SharableTap } from "../src/binder.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const bin = join(here, "..", "..", "node", "target", "debug", "glade-node");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));
const IDS = ["app:selection", "app:notes", "app:activity"];

function fakeAtom(gladeId: string) {
  let v: unknown = "";
  const ls = new Set<() => void>();
  return {
    share: { gladeId, shape: "value" },
    get: () => v,
    set: (x: unknown) => { if (x !== v) { v = x; ls.forEach((l) => l()); } },
    getShareValue: () => v,
    applyShareValue: (x: unknown) => { if (x !== v) { v = x; ls.forEach((l) => l()); } },
    subscribeShare: (l: () => void) => { ls.add(l); return () => ls.delete(l); },
  };
}
function fakeLog(gladeId: string) {
  let list: unknown[] = [];
  return { share: { gladeId, shape: "log" }, get: () => list, applyShareValue: (v: unknown) => (list = v as unknown[]) };
}
function grokOf(...taps: SharableTap[]): GrokLike {
  return { listSharedTaps: () => taps };
}

function startNode(storeDir: string): Promise<{ port: number; child: ChildProcess }> {
  const child = spawn(bin, ["0", storeDir], { stdio: ["ignore", "pipe", "inherit"] });
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("node start timeout")), 8000);
    child.stdout!.on("data", (d: Buffer) => {
      const m = /listening (\d+)/.exec(d.toString());
      if (m) { clearTimeout(t); resolve({ port: Number(m[1]), child }); }
    });
  });
}
function stopNode(child: ChildProcess): Promise<void> {
  return new Promise((resolve) => {
    child.once("exit", () => resolve());
    child.kill();
  });
}
async function until(pred: () => boolean, ms = 3000): Promise<void> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > ms) throw new Error("timeout");
    await new Promise((r) => setTimeout(r, 20));
  }
}

class Participant {
  selection = fakeAtom("app:selection");
  notes = fakeAtom("app:notes");
  activity = fakeLog("app:activity");
  session: Session;
  binder: GripShareBinder;
  client!: GladeClient;
  origin: string;
  private bound = false;
  constructor(origin: string) {
    this.origin = origin;
    this.session = new Session(schema, origin);
    this.binder = new GripShareBinder(grokOf(this.selection, this.notes, this.activity), this.session);
  }
  async connect(url: string): Promise<void> {
    this.client = new GladeClient(schema, this.origin, this.session);
    this.client.onOps = (ops) => this.binder.applyRemote(ops);
    this.binder.onLocalOps = (ops) => this.client.sendOps(ops);
    await this.client.connect(url);
    for (const id of IDS) await this.client.subscribe(SHARE, id);
    if (!this.bound) { this.binder.bind(); this.bound = true; }
    else this.binder.resync(); // reconnect: re-ship ops written while offline
  }
  disconnect(): void { this.client.close(); }
}

test("M-LIMP §11: converge, restart-resume, offline-reconnect, echo", async () => {
  const store = join(here, "..", "..", "node", "target", "mlimp-store");
  rmSync(store, { recursive: true, force: true });
  let node = await startNode(store);

  // --- 1. converge (lww + log) ---
  const A = new Participant("alice");
  const B = new Participant("bob");
  await A.connect(`ws://127.0.0.1:${node.port}`);
  await B.connect(`ws://127.0.0.1:${node.port}`);
  A.selection.set("src/main.rs");
  await until(() => B.selection.get() === "src/main.rs");
  A.binder.appendLog("app:activity", "alice: hello");
  await until(() => (B.activity.get() as unknown[]).length === 1);

  // --- 2. node restart -> resume from store (no data lost) ---
  A.disconnect();
  B.disconnect();
  await stopNode(node.child);
  node = await startNode(store); // same store dir, new port
  const C = new Participant("carol"); // a fresh joiner sees the persisted state
  await C.connect(`ws://127.0.0.1:${node.port}`);
  await until(() => C.selection.get() === "src/main.rs");
  await until(() => (C.activity.get() as unknown[]).length === 1);

  // --- 3. offline write -> reconnect -> reconcile ---
  C.disconnect();
  C.selection.set("offline-pick"); // written locally while offline (not yet shipped)
  await C.connect(`ws://127.0.0.1:${node.port}`); // reconnect resyncs the offline op
  const D = new Participant("dave");
  await D.connect(`ws://127.0.0.1:${node.port}`);
  await until(() => D.selection.get() === "offline-pick"); // dave sees C's offline write

  // --- 4. echo provider answers an EXCHANGE ---
  const res = await D.client.exchange(SHARE, "app:echo", utf8("ping"));
  assert.ok(res.ok, "exchange ok");
  assert.equal(new TextDecoder().decode(res.payload!), "ping");

  C.disconnect();
  D.disconnect();
  await stopNode(node.child);
});
