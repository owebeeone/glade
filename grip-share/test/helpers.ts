// Shared glial-path test plumbing for the post-cutover suite (GC-3 done):
// every binding is a glial mount; these helpers wire real @glade/client-ts
// sessions (and optionally the real rust node) to glial instances through the
// carrier seams the demo uses (ClientBus over a GladeClient; LocalMesh for
// in-process convergence). No grip-share binder — it no longer exists.

import { spawn, type ChildProcess } from "node:child_process";
import { readFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import { Session, type Op } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import {
  feedSession,
  GlialBinder,
  MemoryStoreEngine,
  SessionDestination,
  type Fill,
  type InstanceEvent,
  type OpBus,
  type Route,
  type SessionLike,
  type StoreEngine,
  type WireOp,
} from "@owebeeone/glial-runtime";
import type { BindingDecl } from "@owebeeone/glade-decl";

export const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const bin = join(here, "..", "..", "node", "target", "debug", "glade-node");
export const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

export const dec = new TextDecoder();
export const jsonBytes = (v: unknown) => utf8(JSON.stringify(v ?? null));
export const JSON_PAYLOAD = {
  encode: jsonBytes,
  decode: (b: Uint8Array) => JSON.parse(dec.decode(b)),
};
export interface Codec {
  encode(v: unknown): Uint8Array;
  decode(b: Uint8Array): unknown;
}

export function startNode(tag: string): Promise<{ port: number; child: ChildProcess }> {
  const dir = join(here, "..", "..", "node", "target", `it-${tag}`);
  rmSync(dir, { recursive: true, force: true });
  return startNodeAt(dir);
}
/** Start (or restart) the node on an explicit store dir — restart-resume tests. */
export function startNodeAt(storeDir: string): Promise<{ port: number; child: ChildProcess }> {
  const child = spawn(bin, ["0", storeDir], { stdio: ["ignore", "pipe", "inherit"] });
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
export function stopNode(child: ChildProcess): Promise<void> {
  return new Promise((resolve) => {
    child.once("exit", () => resolve());
    child.kill();
  });
}
export async function until(pred: () => boolean, ms = 3000): Promise<void> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > ms) throw new Error("timeout");
    await new Promise((r) => setTimeout(r, 20));
  }
}

export function decl(
  id: string,
  shape: "value" | "log",
  domain: "account" | "document" = "document",
  zone: "commons" | "private" = "commons",
): BindingDecl {
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

/** The WS carrier as glial's OpBus (the demo's production wiring): publish
 *  ships to the node; inbound node ops fan to every SessionDestination. The
 *  client is re-pointable (reconnect keeps the session + mounts). Records
 *  everything published — wire-byte evidence. */
export class ClientBus implements OpBus {
  published: WireOp[] = [];
  client?: GladeClient;
  private handlers = new Set<(ops: WireOp[]) => void>();
  publish(ops: WireOp[]): void {
    this.published.push(...ops);
    this.client?.sendOps(ops as unknown as Op[]);
  }
  onOps(handler: (ops: WireOp[]) => void): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }
  deliver(ops: Op[]): void {
    for (const h of [...this.handlers]) h(ops as unknown as WireOp[]);
  }
}

/** An in-process op bus modelling the node fan-out (no node spawned): publish
 *  reaches every handler (each SessionDestination filters its route + origin)
 *  and is recorded; `deliver` injects a replay. */
export class LocalMesh implements OpBus {
  captured: WireOp[] = [];
  private handlers = new Set<(ops: WireOp[]) => void>();
  publish(ops: WireOp[]): void {
    this.captured.push(...ops);
    for (const h of [...this.handlers]) h(ops);
  }
  onOps(handler: (ops: WireOp[]) => void): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }
  deliver(ops: WireOp[]): void {
    for (const h of [...this.handlers]) h(ops);
  }
}

export interface MountView {
  events: InstanceEvent[];
  value(): unknown;
  records(): unknown[];
  write(v: unknown): void;
}
export function mountView(
  binder: GlialBinder,
  session: Session,
  bus: OpBus,
  d: BindingDecl,
  fill: Fill,
  route: Route,
  codec: Codec,
): MountView {
  const events: InstanceEvent[] = [];
  const mount = binder.mount(d, fill, (e) => events.push(e), {
    glade: new SessionDestination(session as unknown as SessionLike, bus, route),
  });
  return {
    events,
    value: () => {
      const e = events[events.length - 1];
      return e?.value ? codec.decode(e.value) : undefined;
    },
    records: () => {
      const e = events[events.length - 1];
      return (e?.records ?? []).map((r) => codec.decode(r.payload));
    },
    write: (v: unknown) => mount.instance.write(codec.encode(v)),
  };
}

/** An in-process glial participant (LocalMesh carrier, no node). */
export function localParticipant(
  origin: string,
  mesh: OpBus,
  d: BindingDecl,
  fill: Fill,
  route: Route,
  codec: Codec = JSON_PAYLOAD,
) {
  const session = new Session(schema, origin);
  const binder = new GlialBinder(new MemoryStoreEngine(), origin);
  return { session, binder, ...mountView(binder, session, mesh, d, fill, route, codec) };
}

/** A node-backed glial participant: one mounted binding over the real node,
 *  wired exactly like the demo (feedSession absorbs every inbound op; mount
 *  before subscribe so the replay is not dropped). An injectable engine puts
 *  glial's persistent store (IndexedDB) in the GC-4 slot for reload tests. */
export async function glialParticipant(
  origin: string,
  url: string,
  d: BindingDecl,
  fill: Fill,
  route: Route,
  codec: Codec = JSON_PAYLOAD,
  engine: StoreEngine = new MemoryStoreEngine(),
) {
  const session = new Session(schema, origin);
  const client = new GladeClient(schema, origin, session);
  const bus = new ClientBus();
  bus.client = client;
  client.onOps = (ops) => bus.deliver(ops);
  feedSession(session as unknown as SessionLike, bus);
  const binder = new GlialBinder(engine, origin);
  const view = mountView(binder, session, bus, d, fill, route, codec);
  await client.connect(url);
  await client.subscribe(route.share, route.gladeId, route.key.length ? route.key : undefined);
  return { client, bus, session, binder, engine, ...view };
}
