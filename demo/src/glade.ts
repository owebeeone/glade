// Glade wiring — the gryth toolchain seam in the browser (GC-3 cutover DONE):
// every surface is a glial mount (taps.ts); this module is app glue only —
// connect the WS carrier, subscribe the manifest's zone addresses, expose the
// connection status and the activity write path. The ONE session lives in
// glial.ts; grip-share contributes declaration plumbing only (manifest scope).

import { type Op } from "../../client-ts/src/session.ts";
import { surfaceDecl } from "../../grip-share/src/manifest.ts";
import { WORKSPACE_MANIFEST } from "./manifest";
import { grok, main } from "./runtime";
import { ACTIVITY_TAP } from "./grips";
import { bus, client, session, scope, user } from "./glial";

// Identity + payload types live in glial.ts; re-exported so consumers
// (WorkspacePanel, grips.ts) are untouched.
export { origin, user, doc, type ChatLine } from "./glial";
import type { ChatLine } from "./glial";

// node -> client -> glial bus. Feeding the session is glial's job now, not
// app glue: `feedSession(session, bus)` (wired in glial.ts) absorbs every
// inbound op — truthful heads/resume vectors + own-chain resume on reload —
// and each mounted instance filters its route off the same bus (the semantic
// echo guard folds a reloaded tab's own replay back in live).
client.onOps = (ops) => bus.deliver(ops);

export type GladeStatus = "connecting" | "live" | "offline";
let statusListeners = new Set<(s: GladeStatus) => void>();
let status: GladeStatus = "connecting";
export function onStatus(cb: (s: GladeStatus) => void): () => void {
  statusListeners.add(cb);
  cb(status);
  return () => statusListeners.delete(cb);
}
function setStatus(s: GladeStatus) {
  status = s;
  statusListeners.forEach((l) => l(s));
}

export async function startGladeSync(url: string): Promise<void> {
  try {
    await client.connect(url);
    // subscribe every manifest surface's zone address (commons + our private).
    for (const id of Object.keys(WORKSPACE_MANIFEST.surfaces)) {
      const a = scope.resolve(surfaceDecl(WORKSPACE_MANIFEST, id));
      await client.subscribe(a.share, id, a.key);
    }
    // re-ship anything already in the session (e.g. writes made before the
    // socket opened) — the node dedups by (origin, seq).
    const ops: Op[] = session.dump();
    if (ops.length) client.sendOps(ops);
    setStatus("live");
  } catch (e) {
    // glial persistence keeps local writes; a reconnect would resync them.
    setStatus("offline");
    throw e;
  }
}

/** Append one entry to the shared activity log — a typed ChatLine, attributed
 *  to the participant (not the per-tab origin) — through the glial log
 *  controller (ACTIVITY_TAP), each entry its own op. */
let activityDrip: { get(): { append(entry: unknown): void } | undefined } | undefined;
export function postActivity(text: string): void {
  if (!activityDrip) {
    activityDrip = grok.query(ACTIVITY_TAP, main) as never;
    grok.flush();
  }
  const ctrl = activityDrip!.get();
  if (!ctrl) throw new Error("activity controller not ready (ACTIVITY_TAP unresolved)");
  ctrl.append({ ts: Date.now(), user, text } satisfies ChatLine);
}
