// The glade session (P2) — one origin, its own append-only log, a store of all
// ops it has seen, and the folds over them. The browser folds: a bound tap's
// value is `fold(opsForShare filtered to its glade_id)`. Carrier-agnostic;
// the WS destination (P2.S4) is a thin layer that ships ops to/from a node.

import { foldLog, foldValue, type FoldOp } from "./fold.ts";
import { opHash } from "./hash.ts";
import { Store, type Op } from "./store.ts";
import type { SchemaIndex } from "./taut/schema.ts";

// Op is part of the Session API surface (append/applyRemote/dump) — re-export it.
export type { Op } from "./store.ts";

export class Session {
  private lamport = 0;
  private store: Store;
  private schema: SchemaIndex;
  readonly origin: string;

  constructor(schema: SchemaIndex, origin: string, store?: Store) {
    this.schema = schema;
    this.origin = origin;
    this.store = store ?? new Store(schema);
  }

  /** Append a local op to this origin's chain within a zone (default commons)
   *  and return it. The zone `key` selects the chain — its own seq/prev. */
  append(share: string, gladeId: string, shape: string, payload: Uint8Array, key: Uint8Array = new Uint8Array()): Op {
    const ownLog = this.store.scan(share, gladeId, key, this.origin, -Infinity);
    const last = ownLog[ownLog.length - 1];
    this.lamport += 1;
    const op: Op = {
      share,
      glade_id: gladeId,
      key,
      origin: this.origin,
      seq: last ? last.seq + 1 : 0,
      prev: last ? opHash(this.schema, last as never) : null,
      lamport: this.lamport,
      refs: [],
      shape,
      payload,
    };
    this.store.append(op);
    return op;
  }

  /** Apply ops received from a peer/node; advance the lamport clock. */
  applyRemote(ops: Op[]): void {
    for (const op of ops) {
      try {
        this.store.append(op);
        if (op.lamport > this.lamport) this.lamport = op.lamport;
      } catch {
        // duplicate / equivocation / gap: a real client would surface an Error
        // frame for equivocation; here convergence simply ignores bad ops.
      }
    }
  }

  /** Materialize a bound surface by folding its zone-surface ops (default commons). */
  fold(share: string, gladeId: string, shape: string, key: Uint8Array = new Uint8Array()): Uint8Array | Uint8Array[] | null {
    const ops: FoldOp[] = this.store.opsFor(share, gladeId, key);
    return shape === "log" ? foldLog(ops) : foldValue(ops);
  }

  /** This session's per-origin heads for a share (resume vector). */
  heads(share: string): Map<string, number> {
    return this.store.heads(share);
  }

  /** Ops a peer with `their` heads is missing (gap to ship). */
  missingFor(share: string, their: Map<string, number>): Op[] {
    return this.store.missingFor(share, their);
  }

  /** Hydration: dump / restore the underlying op set. */
  dump(): Op[] {
    return this.store.dump();
  }
  static restore(schema: SchemaIndex, origin: string, ops: Op[]): Session {
    return new Session(schema, origin, Store.load(schema, ops));
  }
}
