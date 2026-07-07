// In-memory per-chain append-log store (P2.S3, local destination) — the TS port
// of the rust node's store, with the same chain/equivocation rules (P1.S4). The
// chain identity is (share, glade_id, key, origin): the zone `key` is part of
// the axis so each zone is independently contiguous (GladeZones.md). Serializable
// for hydration (memory now; IndexedDB is the same shape).

import { bytesEq, hex } from "./bytes.ts";
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
  // chainKey "share\x00gladeId\x00keyHex\x00origin" -> ordered ops
  private logs = new Map<string, Op[]>();
  private schema: SchemaIndex;

  constructor(schema: SchemaIndex) {
    this.schema = schema;
  }

  private chainKey(share: string, gladeId: string, key: Uint8Array, origin: string): string {
    return `${share}\x00${gladeId}\x00${hex(key)}\x00${origin}`;
  }

  /** Append with per-chain checks (mirrors the rust store). */
  append(op: Op): AppendResult {
    const k = this.chainKey(op.share, op.glade_id, op.key, op.origin);
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

  /** Ops for a chain (share, glade_id, key, origin) with seq > fromSeq, in order. */
  scan(share: string, gladeId: string, key: Uint8Array, origin: string, fromSeq: number): Op[] {
    return (this.logs.get(this.chainKey(share, gladeId, key, origin)) ?? []).filter((o) => o.seq > fromSeq);
  }

  /** Per-chain head seq for a share (the resume vector, keyed by chain so two
   *  zones of one origin stay distinct). */
  heads(share: string): Map<string, number> {
    const out = new Map<string, number>();
    for (const [k, log] of this.logs) {
      if (k.startsWith(`${share}\x00`) && log.length) out.set(k, log[log.length - 1].seq);
    }
    return out;
  }

  /** Every op in a zone-surface (share, glade_id, key), across origins — the
   *  fold input for one bound surface. A different zone's ops are never included. */
  opsFor(share: string, gladeId: string, key: Uint8Array): Op[] {
    const prefix = `${share}\x00${gladeId}\x00${hex(key)}\x00`;
    const out: Op[] = [];
    for (const [k, log] of this.logs) {
      if (k.startsWith(prefix)) out.push(...log);
    }
    return out;
  }

  /** Ops the peer holding `their` (chain-keyed) heads is missing — the gap to ship. */
  missingFor(share: string, their: Map<string, number>): Op[] {
    const out: Op[] = [];
    for (const [k, log] of this.logs) {
      if (!k.startsWith(`${share}\x00`)) continue;
      const from = their.get(k) ?? -Infinity;
      out.push(...log.filter((o) => o.seq > from));
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
