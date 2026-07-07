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

/** The default share namespace when a scope doesn't map a domain. */
export const SHARE = "app";

/** grip-core's share hooks, structurally (no grip-core import). */
export interface ShareDecl {
  gladeId: string;
  shape: string;
  authority?: string;
  /** Replicated world (maps to the wire `share` via the scope). */
  domain?: string;
  /** Converging partition (maps to the wire `key` via the scope). */
  zone?: string;
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

/** A surface's wire address: which replicated world, and which zone within it. */
export interface Addr {
  share: string;
  key: Uint8Array;
}

/** Resolves a surface's declared `domain`/`zone` to its wire `(share, key)`.
 *  domain -> share, zone -> key (GladeZones.md). The app supplies the policy
 *  (it knows the current user/document); the binder stays generic. */
export interface Scope {
  resolve(decl: ShareDecl): Addr;
}

/** The trivial scope: one `app` share, the commons zone — preserves the
 *  single-surface M-LIMP behaviour when no zones are declared. */
export const DEFAULT_SCOPE: Scope = {
  resolve: () => ({ share: SHARE, key: new Uint8Array() }),
};

/** Encode/decode a surface's payload to/from the opaque bytes glade carries.
 *  The default is JSON; a *typed* surface (a declared taut message, keyed by
 *  glade id) supplies its own codec. The payload stays opaque to glade — only
 *  the binder and the app know the type. */
export interface PayloadCodec {
  encode(v: unknown): Uint8Array;
  decode(b: Uint8Array): unknown;
}
const JSON_CODEC: PayloadCodec = {
  encode: (v) => utf8(JSON.stringify(v ?? null)),
  decode: (b) => JSON.parse(new TextDecoder().decode(b)),
};

/** Sorted distinct glade ids declared by a grok's shared taps (GQ-6 manifest input). */
export function collectGladeIds(grok: GrokLike): string[] {
  return [...new Set(grok.listSharedTaps().map((t) => t.share?.gladeId).filter(Boolean) as string[])].sort();
}

export class GripShareBinder {
  readonly session: Session;
  private grok: GrokLike;
  private taps = new Map<string, SharableTap>(); // gladeId -> tap
  private shapes = new Map<string, string>(); // gladeId -> shape
  private addrs = new Map<string, Addr>(); // gladeId -> resolved (share, key)
  private applying = false;
  private offs: Array<() => void> = [];

  /** Set by a transport to forward locally-produced ops to peers/node. */
  onLocalOps?: (ops: Op[]) => void;

  // per-surface payload codecs (glade id -> codec); default JSON.
  private codecs: Map<string, PayloadCodec>;
  private scope: Scope;

  constructor(grok: GrokLike, session: Session, codecs?: Map<string, PayloadCodec>, scope: Scope = DEFAULT_SCOPE) {
    this.grok = grok;
    this.session = session;
    this.codecs = codecs ?? new Map();
    this.scope = scope;
  }

  private addrFor(gladeId: string): Addr {
    return this.addrs.get(gladeId) ?? { share: SHARE, key: new Uint8Array() };
  }

  private codecFor(gladeId: string): PayloadCodec {
    return this.codecs.get(gladeId) ?? JSON_CODEC;
  }

  /** Bind every share-declared tap: hydrate from existing state, then wire
   *  local changes to op appends. */
  bind(): void {
    for (const tap of this.grok.listSharedTaps()) {
      const decl = tap.share;
      if (!decl) continue;
      const { gladeId, shape } = decl;
      const addr = this.scope.resolve(decl); // domain+zone -> (share, key)
      this.taps.set(gladeId, tap);
      this.shapes.set(gladeId, shape);
      this.addrs.set(gladeId, addr);

      // hydrate from any already-known folded state
      this.applyFolded(gladeId);

      // value shapes mirror the whole value on every local change; log shapes
      // append discrete entries via appendLog (not a whole-value subscribe).
      if (shape !== "log" && tap.subscribeShare) {
        const off = tap.subscribeShare(() => {
          if (this.applying) return; // echo guard: remote applies must not re-emit
          const payload = this.codecFor(gladeId).encode(tap.getShareValue?.());
          const op = this.session.append(addr.share, gladeId, shape, payload, addr.key);
          this.onLocalOps?.([op]);
        });
        this.offs.push(off);
      }
    }
  }

  /** The distinct zone-surfaces `(share, gladeId, key)` this binder needs the
   *  transport to subscribe — one per bound surface. Call after `bind()`. */
  subscriptions(): Array<{ share: string; gladeId: string; key: Uint8Array }> {
    return [...this.addrs.entries()].map(([gladeId, a]) => ({ share: a.share, gladeId, key: a.key }));
  }

  /** Append one entry to a `log`-shaped binding — each entry is its own op.
   *  The materialized ordered list is folded back onto the bound tap. */
  appendLog(gladeId: string, entry: unknown): Op {
    const shape = this.shapes.get(gladeId) ?? "log";
    const addr = this.addrFor(gladeId);
    const op = this.session.append(addr.share, gladeId, shape, this.codecFor(gladeId).encode(entry), addr.key);
    this.applyFolded(gladeId); // reflect the new entry locally
    this.onLocalOps?.([op]);
    return op;
  }

  /** Re-ship every known op to the transport — e.g. after reconnect, so writes
   *  made while offline reach the node (which dedups by (origin, seq)). */
  resync(): void {
    const ops = this.session.dump();
    if (ops.length) this.onLocalOps?.(ops);
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
    const addr = this.addrs.get(gladeId);
    if (!tap?.applyShareValue || !shape || !addr) return;
    const folded = this.session.fold(addr.share, gladeId, shape, addr.key);
    if (folded == null) return;
    const codec = this.codecFor(gladeId);
    this.applying = true;
    try {
      if (shape === "log") {
        tap.applyShareValue((folded as Uint8Array[]).map((b) => codec.decode(b)));
      } else {
        tap.applyShareValue(codec.decode(folded as Uint8Array));
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
