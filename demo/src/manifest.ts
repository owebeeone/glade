// The gryth-workspace share-space, as data (GladeManifest sketch). This single
// manifest replaces three things that used to be hardcoded and scattered:
//   - per-tap domain/zone/shape decls (was taps.ts)
//   - the domain/zone -> share/key policy (was a hand-written scope in glade.ts)
//   - the glade-id -> payload-type map (was the codecs map in glade.ts)
//
// Today the manifest and a stub grant live client-side. Tomorrow the agent
// publishes the manifest and issues the grant on auth — at which point `self`
// becomes authenticated rather than a URL param, and nothing else changes.

import type { Manifest, Grant } from "../../grip-share/src/manifest.ts";

export const WORKSPACE_MANIFEST: Manifest = {
  manifest: "gryth-workspace",
  version: 1,
  // {self} is identity (agent-bound); {doc} is a session param (client-supplied).
  params: { self: { from: "identity" }, doc: { from: "session" } },
  domains: {
    doc: { share: "doc:{doc}" }, // the open document — its own replicated world
    account: { share: "account:{self}" }, // your account — follows you across docs
  },
  zones: {
    commons: { key: "" }, // everyone in the domain
    private: { key: "self:{self}" }, // keyed to you; never shared
  },
  surfaces: {
    "app:notes": { domain: "doc", zone: "commons", shape: "value", type: "Text" },
    "app:activity": { domain: "doc", zone: "commons", shape: "log", type: "ChatLine" },
    "app:selection": { domain: "doc", zone: "private", shape: "value", type: "Text" },
    "app:status": { domain: "account", zone: "commons", shape: "value", type: "Text" },
  },
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
