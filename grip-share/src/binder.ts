// Grip Share binder (GLP-0005, P3.S2) — the bridge between grip and glade.
//
// It walks a grok's share-declared taps (`listSharedTaps`) and binds each, by
// its glade id, to a glade Session: a local change appends an op; an inbound op
// folds and applies back to the tap. grip-core-agnostic by design — it depends
// only on the structural share hooks (the same ones AtomValueTap implements,
// P3.S1) and the glade Session. Echo control is by an `applying` guard plus the
// substrate's origin attribution: applying a remote value must not re-emit.

import { Session, type Op } from "../../client-ts/src/session.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";

/** The single M-LIMP share namespace (one share for the app). */
export const SHARE = "app";

/** grip-core's share hooks, structurally (no grip-core import). */
export interface ShareDecl {
  gladeId: string;
  shape: string;
  authority?: string;
}
export interface SharableTap {
  share?: ShareDecl;
  getShareValue?(): unknown;
  applyShareValue?(value: unknown): void;
  subscribeShare?(listener: () => void): () => void;
}
export interface GrokLike {
  listSharedTaps(): SharableTap[];
}

// Value <-> opaque payload. JSON is fine: the payload is opaque to glade, and
// the fold only ever compares/sequences whole payloads.
function encodeValue(v: unknown): Uint8Array {
  return utf8(JSON.stringify(v ?? null));
}
function decodeValue(b: Uint8Array): unknown {
  return JSON.parse(new TextDecoder().decode(b));
}

/** Sorted distinct glade ids declared by a grok's shared taps (GQ-6 manifest input). */
export function collectGladeIds(grok: GrokLike): string[] {
  return [...new Set(grok.listSharedTaps().map((t) => t.share?.gladeId).filter(Boolean) as string[])].sort();
}

export class GripShareBinder {
  readonly session: Session;
  private grok: GrokLike;
  private taps = new Map<string, SharableTap>(); // gladeId -> tap
  private shapes = new Map<string, string>(); // gladeId -> shape
  private applying = false;
  private offs: Array<() => void> = [];

  /** Set by a transport to forward locally-produced ops to peers/node. */
  onLocalOps?: (ops: Op[]) => void;

  constructor(grok: GrokLike, session: Session) {
    this.grok = grok;
    this.session = session;
  }

  /** Bind every share-declared tap: hydrate from existing state, then wire
   *  local changes to op appends. */
  bind(): void {
    for (const tap of this.grok.listSharedTaps()) {
      const decl = tap.share;
      if (!decl) continue;
      const { gladeId, shape } = decl;
      this.taps.set(gladeId, tap);
      this.shapes.set(gladeId, shape);

      // hydrate from any already-known folded state
      this.applyFolded(gladeId);

      // value shapes mirror the whole value on every local change; log shapes
      // append discrete entries via appendLog (not a whole-value subscribe).
      if (shape !== "log" && tap.subscribeShare) {
        const off = tap.subscribeShare(() => {
          if (this.applying) return; // echo guard: remote applies must not re-emit
          const payload = encodeValue(tap.getShareValue?.());
          const op = this.session.append(SHARE, gladeId, shape, payload);
          this.onLocalOps?.([op]);
        });
        this.offs.push(off);
      }
    }
  }

  /** Append one entry to a `log`-shaped binding — each entry is its own op.
   *  The materialized ordered list is folded back onto the bound tap. */
  appendLog(gladeId: string, entry: unknown): Op {
    const shape = this.shapes.get(gladeId) ?? "log";
    const op = this.session.append(SHARE, gladeId, shape, encodeValue(entry));
    this.applyFolded(gladeId); // reflect the new entry locally
    this.onLocalOps?.([op]);
    return op;
  }

  /** Ops arriving from a peer/node: store, then re-fold + apply affected taps. */
  applyRemote(ops: Op[]): void {
    this.session.applyRemote(ops);
    for (const gladeId of new Set(ops.map((o) => o.glade_id))) {
      this.applyFolded(gladeId);
    }
  }

  private applyFolded(gladeId: string): void {
    const tap = this.taps.get(gladeId);
    const shape = this.shapes.get(gladeId);
    if (!tap?.applyShareValue || !shape) return;
    const folded = this.session.fold(SHARE, gladeId, shape);
    if (folded == null) return;
    this.applying = true;
    try {
      if (shape === "log") {
        tap.applyShareValue((folded as Uint8Array[]).map((b) => decodeValue(b)));
      } else {
        tap.applyShareValue(decodeValue(folded as Uint8Array));
      }
    } finally {
      this.applying = false;
    }
  }

  dispose(): void {
    for (const off of this.offs) off();
    this.offs = [];
  }
}
