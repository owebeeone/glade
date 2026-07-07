// Manifest-driven scope (GladeManifest sketch) — the share-space spec as DATA.
//
// A `Manifest` (app-static: which surfaces exist and how domain/zone map to the
// wire) plus a `Grant` (per-session, agent-issued: identity + which (share,key)
// you may touch) fully determine each surface's address. The manifest's
// share/key templates carry `{placeholders}`; identity placeholders like
// `{self}` are filled from the grant — never client-chosen — which is exactly
// where the agent/authority plugs in later (GladeZones.md). Today the demo mints
// a stub grant from the URL; nothing here changes when the agent issues it.

import { utf8 } from "../../client-ts/src/bytes.ts";
import type { Addr, PayloadCodec, Scope, ShareDecl } from "./binder.ts";

export interface SurfaceSpec {
  domain: string;
  zone: string;
  shape: string;
  /** Payload type name — selects the codec (see `manifestCodecs`). */
  type: string;
}

export interface Manifest {
  manifest: string;
  version: number;
  /** Where each template placeholder's value comes from — the trust line.
   *  `identity` params are agent-bound (e.g. `self`); `session` params are
   *  client-supplied (e.g. which `doc`) and must be capability-checked. */
  params: Record<string, { from: "identity" | "session" }>;
  /** domain name -> wire `share` template, e.g. `"doc:{doc}"`. */
  domains: Record<string, { share: string }>;
  /** zone name -> wire `key` template, e.g. `"self:{self}"` (`""` = commons). */
  zones: Record<string, { key: string }>;
  /** glade id -> its (domain, zone, shape, payload type). */
  surfaces: Record<string, SurfaceSpec>;
}

export interface Grant {
  /** Agent-authenticated values for `identity` params (e.g. `{ self: "alice" }`). */
  identity: Record<string, string>;
  /** Client-supplied values for `session` params (e.g. `{ doc: "7" }`). */
  session: Record<string, string>;
  /** Resolved `(share, keys)` the session may touch — what an enforcing node
   *  checks each subscribe/write against. (Unenforced on a trusted node.) */
  allow?: Array<{ share: string; keys: string[] }>;
  /** Capability expiry (epoch seconds); `0` = none (trusted-node stub). */
  exp?: number;
}

/** Substitute `{name}` placeholders from `vars`; unknown names collapse to "". */
function fill(tmpl: string, vars: Record<string, string>): string {
  return tmpl.replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? "");
}

/** A binder `Scope` derived from a manifest + a session grant: domain->share,
 *  zone->key, with the grant's identity/session values substituted into the
 *  templates. Because identity comes from the grant (the agent), a private
 *  key (`self:{self}`) is never client-forgeable. */
export function manifestScope(manifest: Manifest, grant: Grant): Scope {
  const vars = { ...grant.identity, ...grant.session };
  return {
    resolve: (decl: ShareDecl): Addr => {
      const spec = manifest.surfaces[decl.gladeId];
      const domain = decl.domain ?? spec?.domain ?? "";
      const zone = decl.zone ?? spec?.zone ?? "";
      const shareTmpl = manifest.domains[domain]?.share ?? "";
      const keyTmpl = manifest.zones[zone]?.key ?? "";
      return { share: fill(shareTmpl, vars), key: utf8(fill(keyTmpl, vars)) };
    },
  };
}

/** The binder `ShareDecl` for a surface, taken from the manifest — so a tap
 *  declares only its glade id and the manifest owns domain/zone/shape. */
export function surfaceDecl(manifest: Manifest, gladeId: string): ShareDecl {
  const s = manifest.surfaces[gladeId];
  if (!s) throw new Error(`manifest has no surface ${gladeId}`);
  return { gladeId, shape: s.shape, domain: s.domain, zone: s.zone };
}

/** Build the binder's per-surface codec map by mapping each surface's `type`
 *  through an app-provided `type -> codec` registry; a type absent from the
 *  registry falls back to the binder's default (JSON). */
export function manifestCodecs(manifest: Manifest, byType: Record<string, PayloadCodec>): Map<string, PayloadCodec> {
  const m = new Map<string, PayloadCodec>();
  for (const [gladeId, s] of Object.entries(manifest.surfaces)) {
    const c = byType[s.type];
    if (c) m.set(gladeId, c);
  }
  return m;
}
