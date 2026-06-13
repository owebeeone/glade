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

export class GladeClient {
  readonly session: Session;
  private schema: SchemaIndex;
  private ws: WebSocket | null = null;
  private subAcks: Array<() => void> = [];

  /** When set, inbound ops are handed here instead of applied to this client's
   *  own session — lets a grip-share binder own the session and folding. */
  onOps?: (ops: Op[]) => void;

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
    });
  }

  private onMessage(bytes: Uint8Array): void {
    const tag = bytes[0];
    const value = codec.decode(this.schema, MSG_BY_TAG[tag], bytes.slice(1)) as Record<string, unknown>;
    if (tag === TAG.Ops) {
      if (this.onOps) this.onOps(value.ops as Op[]);
      else this.session.applyRemote(value.ops as Op[]);
    } else if (tag === TAG.Heads) {
      this.subAcks.shift()?.();
    }
  }

  /** Subscribe to a stream; resolves on the node's Heads ack. */
  subscribe(share: string, gladeId: string): Promise<void> {
    return new Promise((resolve) => {
      this.subAcks.push(resolve);
      this.send(frame(this.schema, TAG.Subscribe, "Subscribe", {
        share, glade_id: gladeId, key: null, from: null,
      }));
    });
  }

  /** Append a local op and ship it to the node (standalone use). */
  append(share: string, gladeId: string, shape: string, payload: Uint8Array): Op {
    const op = this.session.append(share, gladeId, shape, payload);
    this.send(frame(this.schema, TAG.Ops, "Ops", { ops: [op], pri: null }));
    return op;
  }

  /** Ship already-built ops to the node (the binder appends; the client carries). */
  sendOps(ops: Op[]): void {
    this.send(frame(this.schema, TAG.Ops, "Ops", { ops, pri: null }));
  }

  fold(share: string, gladeId: string, shape: string): Uint8Array | Uint8Array[] | null {
    return this.session.fold(share, gladeId, shape);
  }

  close(): void {
    this.ws?.close();
  }

  private send(bytes: Uint8Array): void {
    this.ws?.send(bytes);
  }
}
