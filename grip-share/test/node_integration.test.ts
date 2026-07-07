// Toolchain core (P3.S4 de-risk): grip-share binder converges through the REAL
// rust glade-node over a websocket — value (lww) and log shapes — the workspace
// panel's shared state. Proves rust + glade + grip-share end-to-end (no React).
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
import { GripShareBinder, SHARE, type GrokLike, type Scope, type SharableTap } from "../src/binder.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const bin = join(here, "..", "..", "node", "target", "debug", "glade-node");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

const IDS = ["app:selection", "app:notes", "app:activity"];

function fakeAtom(gladeId: string, initial: unknown) {
  let v = initial;
  const ls = new Set<() => void>();
  return {
    share: { gladeId, shape: "value" },
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
function fakeLog(gladeId: string) {
  let list: unknown[] = [];
  return { share: { gladeId, shape: "log" }, get: () => list, applyShareValue: (v: unknown) => (list = v as unknown[]) };
}
function grokOf(...taps: SharableTap[]): GrokLike {
  return { listSharedTaps: () => taps };
}

function startNode(): Promise<{ port: number; child: ChildProcess }> {
  const dir = join(here, "..", "..", "node", "target", "it-grip-share");
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

async function makeBrowser(origin: string, url: string) {
  const selection = fakeAtom("app:selection", "");
  const notes = fakeAtom("app:notes", "");
  const activity = fakeLog("app:activity");
  const session = new Session(schema, origin);
  const binder = new GripShareBinder(grokOf(selection, notes, activity), session);
  const client = new GladeClient(schema, origin, session);
  client.onOps = (ops) => binder.applyRemote(ops);
  binder.onLocalOps = (ops) => client.sendOps(ops);
  await client.connect(url);
  for (const id of IDS) await client.subscribe(SHARE, id);
  binder.bind();
  return { selection, notes, activity, binder, client };
}

test("workspace panel converges through the rust node (lww + log)", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    const A = await makeBrowser("a", url);
    const B = await makeBrowser("b", url);

    // lww: A picks a selection -> B sees it
    A.selection.set("src/main.rs");
    await until(() => B.selection.get() === "src/main.rs");

    // lww: B edits notes -> A sees it
    B.notes.set("review the resolver");
    await until(() => A.notes.get() === "review the resolver");

    // log: A logs activity, then B logs activity -> both converge in order
    A.binder.appendLog("app:activity", "A opened src/main.rs");
    await until(() => (B.activity.get() as unknown[]).length === 1);
    B.binder.appendLog("app:activity", "B added a note");
    await until(() => (A.activity.get() as unknown[]).length === 2);

    assert.deepEqual(A.activity.get(), B.activity.get());
    assert.equal((A.activity.get() as unknown[]).length, 2);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

// A surface that declares its zone (commons | private). The scope maps the
// zone to a wire key; private => self-keyed, so two users never share it.
function zonedAtom(gladeId: string, zone: string, initial: unknown) {
  return { ...fakeAtom(gladeId, initial), share: { gladeId, shape: "value", domain: "doc", zone } };
}
function scopeFor(user: string): Scope {
  return {
    resolve: (decl) => ({
      share: "doc:1", // the document domain
      key: decl.zone === "private" ? utf8(`self:${user}`) : new Uint8Array(),
    }),
  };
}
async function makeZonedBrowser(user: string, url: string) {
  const selection = zonedAtom("app:selection", "private", ""); // mine, this doc
  const notes = zonedAtom("app:notes", "commons", ""); // everyone, this doc
  const session = new Session(schema, user);
  const binder = new GripShareBinder(grokOf(selection, notes), session, undefined, scopeFor(user));
  const client = new GladeClient(schema, user, session);
  client.onOps = (ops) => binder.applyRemote(ops);
  binder.onLocalOps = (ops) => client.sendOps(ops);
  await client.connect(url);
  binder.bind(); // resolves zone addresses
  for (const s of binder.subscriptions()) await client.subscribe(s.share, s.gladeId, s.key);
  return { selection, notes, binder, client };
}

test("zones: commons converges, private stays per-user", async () => {
  const { port, child } = await startNode();
  const url = `ws://127.0.0.1:${port}`;
  try {
    const alice = await makeZonedBrowser("alice", url);
    const bob = await makeZonedBrowser("bob", url);

    // commons zone (empty key): alice's note reaches bob
    alice.notes.set("shared agenda");
    await until(() => bob.notes.get() === "shared agenda");

    // private zone (self-keyed): each picks a selection; neither crosses
    alice.selection.set("src/main.rs");
    bob.selection.set("Cargo.toml");
    await until(() => bob.selection.get() === "Cargo.toml");
    await until(() => alice.selection.get() === "src/main.rs");
    // let any (erroneous) cross-delivery arrive, then assert isolation held
    await new Promise((r) => setTimeout(r, 120));
    assert.equal(alice.selection.get(), "src/main.rs"); // never bob's "Cargo.toml"
    assert.equal(bob.selection.get(), "Cargo.toml"); // never alice's "src/main.rs"
    assert.equal(alice.notes.get(), "shared agenda"); // commons still shared

    alice.client.close();
    bob.client.close();
  } finally {
    child.kill();
  }
});
