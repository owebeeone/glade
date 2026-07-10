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
import { Session, type Op } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import { GripShareBinder, type GrokLike, type Scope, type SharableTap } from "../src/binder.ts";
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
  constructor(private readonly client: GladeClient) {
    client.onOps = (ops) => this.deliver(ops);
  }
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

/** A glial-era participant: one mounted binding over the real node. */
async function glialParticipant(origin: string, url: string, d: BindingDecl, fill: Fill, route: Route) {
  const session = new Session(schema, origin);
  const client = new GladeClient(schema, origin, session);
  const bus = new ClientBus(client);
  await client.connect(url);
  await client.subscribe(route.share, route.gladeId, route.key.length ? route.key : undefined);
  const glial = new GlialBinder(new MemoryStoreEngine(), origin);
  const events: InstanceEvent[] = [];
  const mount = glial.mount(d, fill, (e) => events.push(e), {
    glade: new SessionDestination(session as unknown as SessionLike, bus, route),
  });
  return {
    client,
    bus,
    events,
    mount,
    /** decoded JSON view of a value surface */
    value: () => {
      const e = events[events.length - 1];
      return e?.value ? JSON.parse(dec.decode(e.value)) : undefined;
    },
    setJson: (v: unknown) => mount.instance.write(jsonBytes(v)),
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
async function binderParticipant(origin: string, url: string, tap: SharableTap, scope: Scope) {
  const session = new Session(schema, origin);
  const grok: GrokLike = { listSharedTaps: () => [tap] };
  const binder = new GripShareBinder(grok, session, undefined, scope);
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
