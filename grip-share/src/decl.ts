// Grip Share — the DECLARATION plumbing that outlived the GC-3 cutover.
//
// The binder (the direct tap↔glade session coupling, GLP-0005 P3.S2) is GONE:
// bindings are glial mounts now (GlialClientRuntime §Boundaries — "grip-share
// shrinks to declaration plumbing; its direct glade coupling is deleted").
// What remains is the share-space VOCABULARY: the structural share declaration
// (`ShareDecl`, the same hooks grip-core's AtomValueTap exposes), the
// domain/zone -> wire (share, key) scope seam, the payload codec shape, and
// the manifest input (`collectGladeIds`, GQ-6). No session, no folding, no
// transport — the compile wall proves it.

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
 *  (it knows the current user/document); the vocabulary stays generic. */
export interface Scope {
  resolve(decl: ShareDecl): Addr;
}

/** The trivial scope: one `app` share, the commons zone. */
export const DEFAULT_SCOPE: Scope = {
  resolve: () => ({ share: SHARE, key: new Uint8Array() }),
};

/** Encode/decode a surface's payload to/from the opaque bytes glade carries.
 *  The default is JSON; a *typed* surface (a declared taut message, keyed by
 *  glade id) supplies its own codec. The payload stays opaque to glade. */
export interface PayloadCodec {
  encode(v: unknown): Uint8Array;
  decode(b: Uint8Array): unknown;
}
export const JSON_CODEC: PayloadCodec = {
  encode: (v) => utf8(JSON.stringify(v ?? null)),
  decode: (b) => JSON.parse(new TextDecoder().decode(b)),
};

/** Sorted distinct glade ids declared by a grok's shared taps (GQ-6 manifest input). */
export function collectGladeIds(grok: GrokLike): string[] {
  return [...new Set(grok.listSharedTaps().map((t) => t.share?.gladeId).filter(Boolean) as string[])].sort();
}
