// In-memory per-(share, origin) append-log store (P2.S3, local destination) —
// the TS port of the rust node's store, with the same chain/equivocation rules
// (P1.S4). Serializable for hydration (memory now; IndexedDB is the same shape).

import { bytesEq } from "./bytes.ts";
import { opHash } from "./hash.ts";
import type { SchemaIndex } from "./taut/schema.ts";

export interface Head {
  origin: string;
  seq: number;
  hash: Uint8Array | null;
}

export interface Op {
  share: string;
  glade_id: string;
  key: Uint8Array;
  origin: string;
  seq: number;
  prev: Uint8Array | null;
  lamport: number;
  refs: Head[];
  shape: string;
  payload: Uint8Array;
}

export type AppendResult = "appended" | "duplicate";

export class Equivocation extends Error {}
export class ChainBreak extends Error {}
export class Gap extends Error {}

export class Store {
  // "share\x00origin" -> ordered ops
  private logs = new Map<string, Op[]>();
  private schema: SchemaIndex;

  constructor(schema: SchemaIndex) {
    this.schema = schema;
  }

  private key(share: string, origin: string): string {
    return `${share}\x00${origin}`;
  }

  /** Append with per-origin chain checks (mirrors the rust store). */
  append(op: Op): AppendResult {
    const k = this.key(op.share, op.origin);
    const log = this.logs.get(k) ?? [];
    const last = log[log.length - 1];
    if (last) {
      if (op.seq <= last.seq) {
        const stored = log.find((o) => o.seq === op.seq);
        if (stored && bytesEq(opHash(this.schema, stored as never), opHash(this.schema, op as never))) {
          return "duplicate";
        }
        if (stored) throw new Equivocation(`forked chain at (${op.origin},${op.seq})`);
        return "duplicate";
      }
      if (op.seq !== last.seq + 1) throw new Gap(`expected ${last.seq + 1}, got ${op.seq}`);
      if (op.prev && !bytesEq(op.prev, opHash(this.schema, last as never))) {
        throw new ChainBreak(`chain break at (${op.origin},${op.seq})`);
      }
    }
    log.push(op);
    this.logs.set(k, log);
    return "appended";
  }

  /** Ops for (share, origin) with seq > fromSeq, in order. */
  scan(share: string, origin: string, fromSeq: number): Op[] {
    return (this.logs.get(this.key(share, origin)) ?? []).filter((o) => o.seq > fromSeq);
  }

  /** Per-origin head seq for a share (the resume vector). */
  heads(share: string): Map<string, number> {
    const out = new Map<string, number>();
    for (const [k, log] of this.logs) {
      const [s, origin] = k.split("\x00");
      if (s === share && log.length) out.set(origin, log[log.length - 1].seq);
    }
    return out;
  }

  /** Every op in a share (all origins), for folding a binding. */
  opsForShare(share: string): Op[] {
    const out: Op[] = [];
    for (const [k, log] of this.logs) {
      if (k.startsWith(`${share}\x00`)) out.push(...log);
    }
    return out;
  }

  /** Ops the peer holding `their` heads is missing (the gap to ship). */
  missingFor(share: string, their: Map<string, number>): Op[] {
    const out: Op[] = [];
    for (const [origin, _seq] of this.heads(share)) {
      const from = their.get(origin) ?? -Infinity;
      out.push(...this.scan(share, origin, from));
    }
    return out;
  }

  /** Flatten every op (hydration dump). */
  dump(): Op[] {
    return [...this.logs.values()].flat();
  }

  /** Rebuild from a dump (re-runs append, so chain checks re-validate). */
  static load(schema: SchemaIndex, ops: Op[]): Store {
    const s = new Store(schema);
    for (const op of ops) s.append(op);
    return s;
  }
}
