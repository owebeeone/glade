// The supplier seam, end-to-end (GLP-0006 P0.S3, GAP-13 closure). The glial
// supplier kit codes against a structural `SupplierSession`; this test proves a
// REAL `GladeClient` satisfies it over a REAL glade-node — the kit's `Supplier`
// serves a DECLARED exchange surface and answers a routed `ExchangeReq` from a
// separate requester session, corr preserved. Until now the kit's serveExchange
// path was only ever exercised against a FAKE (glial `supplier.test.ts`); this
// is the first ws-backed proof that the client-ts hooks (`onExchangeReq` /
// `respondExchange`, added here) close the seam.
//
// Requires the node binary: `cargo build --bin glade-node` in ../../node. The
// node is BOOTED with grazel-app.glade (which declares `service grazel gwz.ops`
// + `workspace ws-razel`) under a TEMP GLADE_HOME/HOME — never the real
// ~/.glade — so gwz.ops is a declared exchange the supplier can attach to.

import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import { GladeClient } from "../src/client.ts";
import { utf8 } from "../src/bytes.ts";
import type { Op } from "../src/store.ts";

import {
  attachSupplier,
  type ExchangeReply,
  type ExchangeRequest,
  type SupplierOp,
  type SupplierSession,
  type SupplierSurface,
} from "../../../glial/src/supplier/index.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const bin = join(here, "..", "..", "node", "target", "debug", "glade-node");
const app = join(here, "..", "..", "node", "..", "apps", "grazel-app.glade");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

const dec = (b: Uint8Array) => new TextDecoder().decode(b);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Boot a glade-node with grazel-app.glade under a temp GLADE_HOME/HOME (the
 *  declared-exchange form). Resolves on the `listening <port>` line. */
function bootNode(): Promise<{ port: number; child: ChildProcess; home: string }> {
  const home = mkdtempSync(join(tmpdir(), "glade-seam-"));
  const child = spawn(
    bin,
    ["--profile", "local", "--name", "seam", "--app", app, "0", join(home, "store")],
    { stdio: ["ignore", "pipe", "inherit"], env: { ...process.env, GLADE_HOME: join(home, "gh"), HOME: join(home, "h") } },
  );
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("node boot timeout")), 10000);
    child.stdout!.on("data", (d: Buffer) => {
      const m = /listening (\d+)/.exec(d.toString());
      if (m) {
        clearTimeout(t);
        resolve({ port: Number(m[1]), child, home });
      }
    });
  });
}

/** The seam adapter: a real `GladeClient` viewed as the kit's structural
 *  `SupplierSession`. The only non-1:1 mapping is `onOps` → `addOpsListener`
 *  (the client keeps `onOps` a settable field for grip-share; suppliers fan
 *  out through the listener set). Everything else is a direct hook. */
class ClientSession implements SupplierSession {
  readonly origin: string;
  constructor(private readonly client: GladeClient) {
    this.origin = client.session.origin;
  }
  subscribe(share: string, gladeId: string, key?: Uint8Array): Promise<void> {
    return this.client.subscribe(share, gladeId, key);
  }
  onExchangeReq(handler: (req: ExchangeRequest) => void): () => void {
    return this.client.onExchangeReq(handler);
  }
  respondExchange(reply: ExchangeReply): void {
    this.client.respondExchange(reply);
  }
  append(share: string, gladeId: string, shape: string, payload: Uint8Array, key?: Uint8Array): SupplierOp {
    return this.client.append(share, gladeId, shape, payload, key) as unknown as SupplierOp;
  }
  onOps(handler: (ops: SupplierOp[]) => void): () => void {
    return this.client.addOpsListener(handler as unknown as (ops: Op[]) => void);
  }
  hello(principal: string): Promise<void> {
    return this.client.hello(principal);
  }
  onDrop(handler: () => void): () => void {
    return this.client.onDrop(handler);
  }
}

const surface: SupplierSurface = { glade_id: { id: "gwz.ops" }, shape: "exchange", share: "ws-razel" };

test("glial Supplier serves a declared exchange over a real GladeClient end-to-end (GAP-13)", async () => {
  const { port, child, home } = await bootNode();
  const url = `ws://127.0.0.1:${port}`;
  const requester = new GladeClient(schema, "requester");
  const supplierClient = new GladeClient(schema, "supplier");
  try {
    await requester.connect(url);
    await supplierClient.connect(url);

    // The kit's Supplier attaches over the real client and serves gwz.ops. The
    // handler is the SUPPLIER's — a distinctive `pong:` prefix so the assertion
    // proves the supplier answered (not the node's echo fallback, which would
    // return the raw payload; and gwz.ops is declared, so echo never runs).
    const sup = attachSupplier(new ClientSession(supplierClient), { principal: "gianni" });
    let seen: string | undefined;
    sup.serveExchange(surface, (req: ExchangeRequest) => {
      seen = dec(req.payload);
      return { payload: utf8(`pong:${dec(req.payload)}`) };
    });

    // Poll the exchange until the provider has attached (Subscribe → attach_
    // provider is async): before attach the node answers ok:false "no authority
    // provider"; once attached, the supplier's handler answers ok:true.
    let res: { ok: boolean; payload?: Uint8Array; error?: string } | undefined;
    for (let i = 0; i < 50 && !(res && res.ok); i++) {
      res = await requester.exchange("ws-razel", "gwz.ops", utf8("gwz.status"));
      if (!res.ok) await sleep(40);
    }

    assert.ok(res && res.ok, `exchange answered ok (last error: ${res?.error})`);
    assert.equal(dec(res!.payload!), "pong:gwz.status", "the SUPPLIER's handler answered, corr routed back");
    assert.equal(seen, "gwz.status", "the supplier handler received the request payload 1:1");

    sup.detachAll();
  } finally {
    requester.close();
    supplierClient.close();
    child.kill();
    await new Promise((r) => child.once("exit", r));
    rmSync(home, { recursive: true, force: true });
  }
});
