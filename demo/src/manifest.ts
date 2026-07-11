// The gryth-workspace surface table, now TYPED (GLP-0006 P0.S5b). The stringly
// table + per-glade-id lookups are gone: consumers reference `M.notes` /
// `M.selection` / `M.activity` / `M.status` — each a typed `Surface` handle (a
// frozen `BindingDecl`) — never the string "app:notes". An undefined or typo'd
// surface (`M.nope`) is a TypeScript BUILD ERROR by construction: the declared-
// surface compile wall, adopted from glial's `defineManifest` (P0.S5a).
//
// The grip-share manifest plumbing is UNCHANGED and still working: the
// `WORKSPACE_MANIFEST` below keeps the domain/zone -> share/key POLICY
// (templates), and `manifestScope` / `Grant` / `stubGrant` resolve each session's
// concrete wire address exactly as before (glial.ts). The typed handle CARRIES
// the same data (glade id, shape, domain anchor, zone), so the scope resolves off
// the handle — which is why the surfaces table moved OUT of `WORKSPACE_MANIFEST`
// into `M`. The one policy edit: the `domains` map is keyed by the canonical
// `DomainAnchor` ("document"/"account") the handle already uses (was "doc"/
// "account"); the share-template VALUES are identical — see `dev-docs/
// GladeZones.md` (Typed manifest note, 2026-07-12).

import { defineManifest, type Surface } from "@owebeeone/glial-runtime/manifest";
import type { Manifest, Grant } from "../../grip-share/src/manifest.ts";

/** The app's declared surfaces — the legible app surface, as typed handles.
 *  `share` is the domain's share TEMPLATE (the grant resolves the concrete
 *  replicated world per session; grazel config resolves it later). `retention`
 *  is `from_cursor` (the demo's log/value replay contract). */
export const M = defineManifest({
  // ACCOUNT domain, commons — your status; follows you across documents.
  status: {
    id: "app:status", shape: "value", share: "account:{self}",
    domain: "account", zone: "commons", retention: { policy: "from_cursor", ttl_ms: null },
  },
  // DOCUMENT domain, private — your selection; keyed to you, never shared.
  selection: {
    id: "app:selection", shape: "value", share: "doc:{doc}",
    domain: "document", zone: "private", retention: { policy: "from_cursor", ttl_ms: null },
  },
  // DOCUMENT domain, commons — the document's shared notes.
  notes: {
    id: "app:notes", shape: "value", share: "doc:{doc}",
    domain: "document", zone: "commons", retention: { policy: "from_cursor", ttl_ms: null },
  },
  // DOCUMENT domain, commons (a log) — the document's activity feed (ChatLine).
  activity: {
    id: "app:activity", shape: "log", share: "doc:{doc}",
    domain: "document", zone: "commons", retention: { policy: "from_cursor", ttl_ms: null },
  },
});
export type { Surface };

/** The share-space POLICY, as data: domain -> wire `share`, zone -> wire `key`.
 *  Identity placeholders (`{self}`) fill from the grant (agent-bound); session
 *  placeholders (`{doc}`) fill client-side. Surfaces now live in `M` — the scope
 *  resolves off each handle's domain/zone, so the `surfaces` table is empty. */
export const WORKSPACE_MANIFEST: Manifest = {
  manifest: "gryth-workspace",
  version: 1,
  params: { self: { from: "identity" }, doc: { from: "session" } },
  domains: {
    document: { share: "doc:{doc}" }, // the open document — its own replicated world
    account: { share: "account:{self}" }, // your account — follows you across docs
  },
  zones: {
    commons: { key: "" }, // everyone in the domain
    private: { key: "self:{self}" }, // keyed to you; never shared
  },
  surfaces: {}, // moved to `M` (typed handles); scope resolves off the handle.
};

/** STUB grant — minted client-side from the URL. The real one is agent-issued
 *  on auth, where `identity.self` is authenticated (not `?user=`). The `allow`
 *  list is what a future enforcing node checks each subscribe/write against. */
export function stubGrant(user: string, doc: string): Grant {
  return {
    identity: { self: user },
    session: { doc },
    allow: [
      { share: `doc:${doc}`, keys: ["", `self:${user}`] },
      { share: `account:${user}`, keys: [""] },
    ],
    exp: 0,
  };
}
