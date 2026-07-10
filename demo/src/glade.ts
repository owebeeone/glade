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

// node -> client -> (session + glial bus). The session sees EVERY inbound op:
// that keeps its heads/resume vectors truthful AND lets a fresh page session
// resume its own chain off the node replay (own-origin ops are echo-guarded
// out of the instance path, so without this a reload would restart seq at 0 —
// a forked chain the node rightly drops). Duplicates dedup in the session
// store. Each mounted instance then filters its own route off the bus.
client.onOps = (ops) => {
  session.applyRemote(ops);
  bus.deliver(ops);
};

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
