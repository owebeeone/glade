// WS destination (P2.S4) — connects a Session to a glade node over a websocket
// using Node's built-in WebSocket. Frames are `[FrameType tag][CBOR body]`
// (the frozen wire). Inbound Ops fold into the session; Subscribe is acked by a
// Heads frame. Carrier detail only — the convergence lives in the Session.

import { Session } from "./session.ts";
import * as codec from "./taut/codec.ts";
import type { SchemaIndex } from "./taut/schema.ts";
import type { Op } from "./store.ts";

const TAG = {
  Hello: 0, Welcome: 1, Subscribe: 2, Unsubscribe: 3, Ops: 4, Heads: 5,
  ExchangeReq: 6, ExchangeRes: 7, ChannelOpen: 8, ChannelData: 9, ChannelClose: 10,
  Chunk: 11, Error: 12,
} as const;

const MSG_BY_TAG: Record<number, string> = {
  0: "Hello", 1: "Welcome", 2: "Subscribe", 3: "Unsubscribe", 4: "Ops", 5: "Heads",
  6: "ExchangeReq", 7: "ExchangeRes", 8: "ChannelOpen", 9: "ChannelData", 10: "ChannelClose",
  11: "Chunk", 12: "Error",
};

function frame(schema: SchemaIndex, tag: number, message: string, value: unknown): Uint8Array {
  const body = codec.encode(schema, message, value as never);
  const out = new Uint8Array(1 + body.length);
  out[0] = tag;
  out.set(body, 1);
  return out;
}

/** An inbound directed request routed to this session as the attached provider —
 *  the wire `ExchangeReq` (tag 6), decoded. `corr` MUST be echoed 1:1 in the
 *  answer. Structurally the glial supplier kit's `ExchangeRequest`. */
export interface InboundExchangeReq {
  share: string;
  glade_id: string;
  corr: string;
  payload: Uint8Array;
}

/** The provider's answer to one request, shipped as a tag-7 `ExchangeRes` with
 *  `corr` preserved. Structurally the glial supplier kit's `ExchangeReply`. */
export interface OutboundExchangeRes {
  corr: string;
  ok: boolean;
  payload?: Uint8Array;
  error?: string;
}

export class GladeClient {
  readonly session: Session;
  private schema: SchemaIndex;
  private ws: WebSocket | null = null;
  private subAcks: Array<() => void> = [];
  private welcomeAcks: Array<() => void> = [];

  /** When set, inbound ops are handed here instead of applied to this client's
   *  own session — lets a grip-share binder own the session and folding. */
  onOps?: (ops: Op[]) => void;

  private exCorr = 0;
  private exWaiters = new Map<string, (r: { ok: boolean; payload?: Uint8Array; error?: string }) => void>();

  // Fan-out registries (GLP-0006 P0.S3 supplier seam): a supplier serving
  // several surfaces over one session needs many listeners, so these are Sets
  // returning an unsubscribe — unlike the single `onOps` field (kept for
  // grip-share/demo). The field owns folding when set; listeners are additive.
  private opsListeners = new Set<(ops: Op[]) => void>();
  private exReqHandlers = new Set<(req: InboundExchangeReq) => void>();
  private dropHandlers = new Set<() => void>();
  /** A caller-initiated `close()` must NOT look like a link drop (no reattach). */
  private closing = false;

  constructor(schema: SchemaIndex, origin: string, session?: Session) {
    this.schema = schema;
    this.session = session ?? new Session(schema, origin);
  }

  connect(url: string): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.binaryType = "arraybuffer";
      this.ws = ws;
      ws.onopen = () => resolve();
      ws.onerror = () => reject(new Error("websocket error"));
      ws.onmessage = (ev: MessageEvent) => this.onMessage(new Uint8Array(ev.data as ArrayBuffer));
      ws.onclose = () => {
        // A link drop (node death, network loss) — fire drop listeners so a
        // supplier reattaches. A deliberate `close()` is not a drop.
        if (!this.closing) for (const h of [...this.dropHandlers]) h();
      };
    });
  }

  private onMessage(bytes: Uint8Array): void {
    const tag = bytes[0];
    const value = codec.decode(this.schema, MSG_BY_TAG[tag], bytes.slice(1)) as Record<string, unknown>;
    if (tag === TAG.Ops) {
      const ops = value.ops as Op[];
      // The `onOps` field keeps its exact contract (grip-share owns folding
      // when set; else the session folds). Op listeners are an additive
      // fan-out for suppliers serving shares — byte-for-byte for the field.
      if (this.onOps) this.onOps(ops);
      else this.session.applyRemote(ops);
      for (const h of [...this.opsListeners]) h(ops);
    } else if (tag === TAG.Heads) {
      this.subAcks.shift()?.();
    } else if (tag === TAG.Welcome) {
      this.welcomeAcks.shift()?.();
    } else if (tag === TAG.ExchangeReq) {
      // Inbound directed request: this session is the attached provider. The
      // node already routed it here (it decodes-and-drops without a handler),
      // so surface it to every registered provider handler, corr intact.
      const req: InboundExchangeReq = {
        share: value.share as string,
        glade_id: value.glade_id as string,
        corr: value.corr as string,
        payload: value.payload as Uint8Array,
      };
      for (const h of [...this.exReqHandlers]) h(req);
    } else if (tag === TAG.ExchangeRes) {
      this.exWaiters.get(value.corr as string)?.({
        ok: value.ok as boolean,
        payload: value.payload as Uint8Array | undefined,
        error: value.error as string | undefined,
      });
      this.exWaiters.delete(value.corr as string);
    }
  }

  /** Send the wire Hello, optionally BINDING this session to a principal
   *  (principals minimal, GLP-0006 P0.S7): the node auto-appends an unknown
   *  principal to dir.principals — identity as data, nothing enforced.
   *  Resolves on the node's Welcome. Entirely optional: sessions that never
   *  call it keep origin-as-identity, byte-for-byte the old behavior. */
  hello(principal?: string): Promise<void> {
    return new Promise((resolve) => {
      this.welcomeAcks.push(resolve);
      this.send(frame(this.schema, TAG.Hello, "Hello", {
        session: this.session.origin, protocol: 1,
        principal: principal ?? null, capability: null, heads: [],
      }));
    });
  }

  /** Subscribe to a zone-surface (share, gladeId, key); resolves on the node's
   *  Heads ack. An absent/empty key is the commons zone. */
  subscribe(share: string, gladeId: string, key?: Uint8Array): Promise<void> {
    return new Promise((resolve) => {
      this.subAcks.push(resolve);
      this.send(frame(this.schema, TAG.Subscribe, "Subscribe", {
        share, glade_id: gladeId, key: key && key.length ? key : null, from: null,
      }));
    });
  }

  /** Append a local op in a zone (default commons) and ship it to the node. */
  append(share: string, gladeId: string, shape: string, payload: Uint8Array, key?: Uint8Array): Op {
    const op = this.session.append(share, gladeId, shape, payload, key);
    this.send(frame(this.schema, TAG.Ops, "Ops", { ops: [op], pri: null }));
    return op;
  }

  /** Ship already-built ops to the node (the binder appends; the client carries). */
  sendOps(ops: Op[]): void {
    this.send(frame(this.schema, TAG.Ops, "Ops", { ops, pri: null }));
  }

  /** Register an additional inbound-ops listener (fan-out); returns an
   *  unsubscribe. Complements the single `onOps` field: a supplier serving
   *  several surfaces over one session registers one listener per surface. */
  addOpsListener(handler: (ops: Op[]) => void): () => void {
    this.opsListeners.add(handler);
    return () => this.opsListeners.delete(handler);
  }

  /** Surface inbound directed requests (tag-6 `ExchangeReq`) to a provider
   *  handler; returns an unsubscribe. Pairs with {@link respondExchange} — this
   *  session is THE attached provider once it has Subscribed a declared exchange
   *  surface (`exchange.rs::attach_provider`). */
  onExchangeReq(handler: (req: InboundExchangeReq) => void): () => void {
    this.exReqHandlers.add(handler);
    return () => this.exReqHandlers.delete(handler);
  }

  /** Answer a directed request: ship a tag-7 `ExchangeRes`, `corr` preserved
   *  1:1 (the node relays it to the recorded requester). Failure is data —
   *  pass `ok:false` with an `error`, never hang. */
  respondExchange(res: OutboundExchangeRes): void {
    this.send(frame(this.schema, TAG.ExchangeRes, "ExchangeRes", {
      corr: res.corr,
      ok: res.ok,
      payload: res.payload ?? null,
      error: res.error ?? null,
    }));
  }

  /** Register a link-drop listener (ws close that was NOT a deliberate
   *  `close()`); returns an unsubscribe. Drives supplier reattach-on-drop. */
  onDrop(handler: () => void): () => void {
    this.dropHandlers.add(handler);
    return () => this.dropHandlers.delete(handler);
  }

  /** A directed request/response to a provider (e.g. the echo provider). */
  exchange(share: string, gladeId: string, payload: Uint8Array): Promise<{ ok: boolean; payload?: Uint8Array; error?: string }> {
    const corr = `c${++this.exCorr}`;
    return new Promise((resolve) => {
      this.exWaiters.set(corr, resolve);
      this.send(frame(this.schema, TAG.ExchangeReq, "ExchangeReq", { share, glade_id: gladeId, corr, payload }));
    });
  }

  fold(share: string, gladeId: string, shape: string, key?: Uint8Array): Uint8Array | Uint8Array[] | null {
    return this.session.fold(share, gladeId, shape, key);
  }

  close(): void {
    this.closing = true;
    this.ws?.close();
  }

  private send(bytes: Uint8Array): void {
    this.ws?.send(bytes);
  }
}
