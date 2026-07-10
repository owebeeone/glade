// Glade wiring — the gryth toolchain seam in the browser.
//
// GC-3 cutover in progress: the session/client/bus live in glial.ts (the ONE
// session both paths share). Surfaces not yet cut over stay bound by the
// grip-share binder below; cut-over surfaces are glial mounts (taps.ts) and the
// binder no longer sees them (they stop being share-declared taps).

import { Session, type Op } from "../../client-ts/src/session.ts";
import { GripShareBinder } from "../../grip-share/src/binder.ts";
import { manifestCodecs, surfaceDecl } from "../../grip-share/src/manifest.ts";
import { WORKSPACE_MANIFEST } from "./manifest";
import { grok } from "./runtime";
import { bus, client, session, scope, CODECS_BY_TYPE, user } from "./glial";

// Identity + payload types moved to glial.ts; re-exported so consumers
// (WorkspacePanel, grips.ts) are untouched.
export { origin, user, doc, type ChatLine } from "./glial";
import type { ChatLine } from "./glial";

const codecs = manifestCodecs(WORKSPACE_MANIFEST, CODECS_BY_TYPE);

const binder = new GripShareBinder(
  { listSharedTaps: () => grok.listSharedTaps() as never },
  session as Session,
  codecs,
  scope,
);

// node -> client -> (glial bus + binder); binder -> client -> node.
client.onOps = (ops) => {
  bus.deliver(ops);
  binder.applyRemote(ops);
};
binder.onLocalOps = (ops) => client.sendOps(ops);

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

let bound = false;
function ensureBound(): void {
  if (!bound) {
    binder.bind(); // resolves each remaining surface's zone address
    bound = true;
  }
}

export async function startGladeSync(url: string): Promise<void> {
  try {
    await client.connect(url);
    ensureBound();
    // subscribe every manifest surface's zone address (commons + our private) —
    // the same set the binder used to enumerate, now manifest-driven so it
    // covers cut-over surfaces too.
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
    setStatus("offline");
    ensureBound(); // local appends still work; reconnect would resync.
    throw e;
  }
}

/** Append one entry to the shared activity log — a typed ChatLine, attributed
 *  to the participant (not the per-tab origin). */
export function postActivity(text: string): void {
  binder.appendLog("app:activity", { ts: Date.now(), user, text } satisfies ChatLine);
}
