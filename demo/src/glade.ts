// Glade wiring — the gryth toolchain seam in the browser:
//   grok share-taps -> grip-share binder -> glade client -> WS -> rust node
// The binder owns the session and folding; the client is pure transport.

import gladeIr from "../../../taut/corpus/glade.ir.json";
import workspaceIr from "../ir/workspace.ir.json";
import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import * as tautCodec from "../../client-ts/src/taut/codec.ts";
import { Session } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { GripShareBinder, type PayloadCodec } from "../../grip-share/src/binder.ts";
import { manifestScope, manifestCodecs } from "../../grip-share/src/manifest.ts";
import { WORKSPACE_MANIFEST, stubGrant } from "./manifest";
import { grok } from "./runtime";

const schema = loadSchema(gladeIr as never);

// The app surface types (taut) — the declared payload for typed surfaces.
const appSchema = loadSchema(workspaceIr as never);

/** One activity-log entry — the declared `ChatLine` taut message. */
export interface ChatLine {
  ts: number;
  user: string;
  text: string;
}

/** taut codecs keyed by the manifest's surface `type`. The manifest maps each
 *  glade id to a type; `manifestCodecs` turns this into the binder's per-id
 *  codec map. Types absent here (e.g. "Text") use the binder's default JSON. */
const CODECS_BY_TYPE: Record<string, PayloadCodec> = {
  ChatLine: {
    encode: (v) => tautCodec.encode(appSchema, "ChatLine", v as never),
    decode: (b) => tautCodec.decode(appSchema, "ChatLine", b),
  },
};

// Stable per-tab origin across reloads, so the node resumes our own log rather
// than treating every reload as a new participant.
function stableOrigin(): string {
  const key = "glade-origin";
  let o = localStorage.getItem(key);
  if (!o) {
    o = Math.random().toString(36).slice(2, 8);
    localStorage.setItem(key, o);
  }
  return o;
}

const params = new URLSearchParams(location.search);
export const origin = stableOrigin();
/** The participant identity — keys the private zone. Defaults to this tab, so
 *  two tabs are different users (private selection stays separate); `?user=alice`
 *  on both makes them the same user (private selection converges). */
export const user = params.get("user") ?? origin;
/** The open document — its own replicated world. `?doc=7` joins another. */
export const doc = params.get("doc") ?? "1";

// The share-space (domain/zone -> share/key policy + surfaces) comes from the
// manifest; identity (`self`) and the open doc come from the grant. Swap
// `stubGrant` for an agent-issued grant and `self` stops being client-settable —
// nothing else here changes (GladeZones.md, GladeManifest sketch).
const grant = stubGrant(user, doc);
const scope = manifestScope(WORKSPACE_MANIFEST, grant);
const codecs = manifestCodecs(WORKSPACE_MANIFEST, CODECS_BY_TYPE);

const session = new Session(schema, origin);
const binder = new GripShareBinder(
  { listSharedTaps: () => grok.listSharedTaps() as never },
  session,
  codecs,
  scope,
);
const client = new GladeClient(schema, origin, session);

// node -> client -> binder (fold + apply to taps); binder -> client -> node
client.onOps = (ops) => binder.applyRemote(ops);
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
    binder.bind(); // resolves each surface's zone address (domain+zone -> share+key)
    bound = true;
  }
}

export async function startGladeSync(url: string): Promise<void> {
  try {
    await client.connect(url);
    ensureBound();
    // subscribe to exactly the zone-surfaces we bound (commons + our private)
    for (const s of binder.subscriptions()) await client.subscribe(s.share, s.gladeId, s.key);
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
