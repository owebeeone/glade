// Glade wiring — the gryth toolchain seam in the browser:
//   grok share-taps -> grip-share binder -> glade client -> WS -> rust node
// The binder owns the session and folding; the client is pure transport.

import gladeIr from "../../../taut/corpus/glade.ir.json";
import workspaceIr from "../ir/workspace.ir.json";
import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import * as tautCodec from "../../client-ts/src/taut/codec.ts";
import { Session } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { GripShareBinder, SHARE, type PayloadCodec } from "../../grip-share/src/binder.ts";
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

/** glade id -> typed taut codec. app:activity carries ChatLine; the rest
 *  fall back to the binder's default JSON codec. (This map is what the .glade
 *  compiler will eventually generate from the surface declarations.) */
const codecs = new Map<string, PayloadCodec>([
  [
    "app:activity",
    {
      encode: (v) => tautCodec.encode(appSchema, "ChatLine", v as never),
      decode: (b) => tautCodec.decode(appSchema, "ChatLine", b),
    },
  ],
]);

const GLADE_IDS = ["app:selection", "app:notes", "app:activity"];

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

export const origin = stableOrigin();

const session = new Session(schema, origin);
const binder = new GripShareBinder(
  { listSharedTaps: () => grok.listSharedTaps() as never },
  session,
  codecs,
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

export async function startGladeSync(url: string): Promise<void> {
  try {
    await client.connect(url);
    for (const id of GLADE_IDS) await client.subscribe(SHARE, id);
    binder.bind();
    setStatus("live");
  } catch (e) {
    setStatus("offline");
    // local appends still work; reconnect would resync.
    binder.bind();
    throw e;
  }
}

/** Append one entry to the shared activity log — a typed ChatLine. */
export function postActivity(text: string): void {
  binder.appendLog("app:activity", { ts: Date.now(), user: origin, text } satisfies ChatLine);
}
