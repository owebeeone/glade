// WS destination (P2.S4) — connects a Session to a glade node over a websocket
// using Node's built-in WebSocket. Frames are `[FrameType tag][CBOR body]`
// (the frozen wire). Inbound Ops fold into the session; Subscribe is acked by a
// Heads frame, and each op sent is answered by an Error frame naming it by hash
// (answers.ts). Carrier detail only — the convergence lives in the Session.

import { Session } from "./session.ts";
import * as codec from "./taut/codec.ts";
import type { SchemaIndex } from "./taut/schema.ts";
import { zoneKey, type Op } from "./store.ts";
import { requireOpShape } from "./shapes.ts";
import { decodeSwmrAction } from "./swmr.ts";
import { Answers, type OpOutcome } from "./answers.ts";
import { hex } from "./bytes.ts";
import { opHash } from "./hash.ts";

// The node's answer to an op is part of the client API surface — re-export it.
export type { OpOutcome } from "./answers.ts";

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

/** Every op must be one this client carries, before any of them is sent. */
function requireShippable(ops: Op[], operation: string): void {
  for (const op of ops) {
    const shape = requireOpShape(op.shape, operation);
    if (shape === "swmr") {
      decodeSwmrAction(op.payload);
    }
  }
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

  // The node answers every op this client sends (GladeSubstrateV1 §6, R1):
  // the ops wait here by hash, and refusals and unplaced ops are told.
  private answers = new Answers();
  private refusedListeners = new Set<(outcome: OpOutcome) => void>();
  private unplacedListeners = new Set<(outcome: OpOutcome) => void>();
  /** Each zone's next resend of its unplaced ops (W5). */
  private resendTimers = new Map<string, ReturnType<typeof setTimeout>>();

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
        if (this.ws === ws) {
          this.ended();
        }
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
      // Reject the whole batch before session storage or consumer callbacks.
      for (const op of ops) {
        const shape = requireOpShape(op.shape, "receive");
        if (shape === "swmr") decodeSwmrAction(op.payload);
      }
      // The `onOps` field keeps its exact contract (grip-share owns folding
      // when set; else the session folds). Op listeners are an additive
      // fan-out for suppliers serving shares — byte-for-byte for the field.
      if (this.onOps) this.onOps(ops);
      else this.session.applyRemote(ops);
      for (const h of [...this.opsListeners]) h(ops);
    } else if (tag === TAG.Heads) {
      // An accepted subscribe's ack names its zone, a refused one's none (R6).
      // The zone's chain resumes after a refusal, and its unplaced ops go
      // again (answer 4, W5): Step 3.4 moves both to the end of the replay.
      for (const s of value.streams as Array<{ share: string; glade_id: string; key: Uint8Array }>) {
        this.session.resume(s.share, s.glade_id, s.key);
        this.resend(zoneKey(s.share, s.glade_id, s.key));
      }
      this.subAcks.shift()?.();
    } else if (tag === TAG.Error) {
      // An op's status names it by hash (R1). An Error with no corr is a
      // refused subscribe's reason (R6), which Step 3.4 reads.
      if (value.corr !== null) {
        this.onStatus(value.corr as string, value.code as string, value.message as string);
      }
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
    this.ship([op]);
    return op;
  }

  /** Ship already-built ops to the node (the binder appends; the client carries). */
  sendOps(ops: Op[]): void {
    requireShippable(ops, "sendOps");
    this.ship(ops);
  }

  /** `append`, resolving with the node's answer as data (R1, R7). It fails at
   *  once, and appends nothing, when no socket is open. */
  async appendOutcome(share: string, gladeId: string, shape: string, payload: Uint8Array, key?: Uint8Array): Promise<OpOutcome> {
    this.requireOpen();
    const op = this.session.append(share, gladeId, shape, payload, key);
    return new Promise((resolve) => this.ship([op], [resolve]));
  }

  /** `sendOps`, resolving with the node's answer to each op, in order (R1,
   *  R7). It fails at once when no socket is open. */
  async sendOpsOutcome(ops: Op[]): Promise<OpOutcome[]> {
    this.requireOpen();
    requireShippable(ops, "sendOpsOutcome");
    const waiters: Array<(outcome: OpOutcome) => void> = [];
    const outcomes = ops.map((_, i) => new Promise<OpOutcome>((resolve) => {
      waiters[i] = resolve;
    }));
    this.ship(ops, waiters);
    return Promise.all(outcomes);
  }

  /** Report every refusal of an op this client sent; returns an unsubscribe.
   *  A session the client owns also drops the op and its chain's later ops,
   *  while a session a binder owns (the `onOps` field set) is only told. With
   *  no listener, a refusal goes to `console.warn`. */
  onRefused(handler: (outcome: OpOutcome) => void): () => void {
    this.refusedListeners.add(handler);
    return () => this.refusedListeners.delete(handler);
  }

  /** Report, once, each op the node could not place (W5): the client keeps
   *  it and its chain, and sends them again. Returns an unsubscribe. With no
   *  listener, it goes to `console.warn`. */
  onUnplaced(handler: (outcome: OpOutcome) => void): () => void {
    this.unplacedListeners.add(handler);
    return () => this.unplacedListeners.delete(handler);
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

  private isOpen(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private requireOpen(): void {
    if (!this.isOpen()) {
      throw new Error("glade client: not connected");
    }
  }

  /** Send ops in one frame, and keep each until its status names it (R1). An
   *  op sent with no socket open reaches no node, so it is not kept. */
  private ship(ops: Op[], waiters: Array<(outcome: OpOutcome) => void> = []): void {
    const open = this.isOpen();
    this.send(frame(this.schema, TAG.Ops, "Ops", { ops, pri: null }));
    if (!open) {
      return;
    }
    ops.forEach((op, i) => this.answers.sent(op, hex(opHash(this.schema, op as never)), waiters[i]));
  }

  /** One op's status (R1): a refusal is told, and a session the client owns
   *  drops the op and its chain's tail (answer 4); an op not placed is told
   *  once, and kept to send again (W5). */
  private onStatus(corr: string, code: string, message: string): void {
    const answered = this.answers.status(corr, code, message);
    if (!answered) {
      return;
    }
    if (answered.refused) {
      if (!this.onOps) {
        this.session.refuse(answered.outcome.op);
      }
      this.tell(this.refusedListeners, answered.outcome, "refused");
    }
    if (answered.unplaced) {
      this.tell(this.unplacedListeners, answered.outcome, "not placed, and kept to send again");
    }
    this.pace(answered.zone);
  }

  /** To the listeners, or with none, to the console: the desk then shows it
   *  without a change of its own. */
  private tell(listeners: Set<(outcome: OpOutcome) => void>, o: OpOutcome, what: string): void {
    if (listeners.size === 0) {
      console.warn(`[glade] op ${what}: ${o.code} (${o.op.share}, ${o.op.glade_id}, ${o.op.origin}, seq ${o.op.seq}): ${o.message}`);
      return;
    }
    for (const h of [...listeners]) {
      h(o);
    }
  }

  /** Send a zone's unplaced ops again, in their chains' order (W5). */
  private resend(zone: string): void {
    const ops = this.answers.unplacedIn(zone);
    if (ops.length > 0 && this.isOpen()) {
      this.ship(ops);
    }
  }

  /** While a zone has unplaced ops, a timer sends them again on W5's backoff;
   *  once none remain, it stops. */
  private pace(zone: string): void {
    const timer = this.resendTimers.get(zone);
    if (this.answers.unplacedIn(zone).length === 0) {
      clearTimeout(timer);
      this.resendTimers.delete(zone);
    } else if (timer === undefined) {
      this.resendTimers.set(zone, setTimeout(() => {
        this.resendTimers.delete(zone);
        if (this.isOpen()) {
          this.resend(zone);
          this.pace(zone);
        }
      }, this.answers.nextResend(zone)));
    }
  }

  /** The connection ended: its waiting ops' fates are unknown (R7), and no
   *  resend runs until an ack on a new one (W5). */
  private ended(): void {
    this.answers.ended();
    for (const t of this.resendTimers.values()) {
      clearTimeout(t);
    }
    this.resendTimers.clear();
  }
}
