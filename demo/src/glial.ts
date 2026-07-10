// Glial wiring seam (Lane T step 3b — the GC-3 per-binding cutover).
//
// ONE session + ONE WS client for the whole app, glial's instance registry, and
// the manifest-derived decl/fill/route/codec for each surface. During the
// cutover the remaining grip-share binder shares this session; when the last
// binding moves, glial holds the only session reference (GlialClientRuntime
// §Boundaries: grip-share shrinks to declaration plumbing).
//
// Wire-byte compatibility is BY CONSTRUCTION: the route (share, key) comes from
// the SAME manifestScope the grip-share binder used, the payload codecs are the
// SAME (JSON default, taut ChatLine), and ops are minted by the SAME
// client-ts Session — nothing about the stored bytes changes.

import gladeIr from "../../../taut/corpus/glade.ir.json";
import workspaceIr from "../ir/workspace.ir.json";
import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import * as tautCodec from "../../client-ts/src/taut/codec.ts";
import { Session, type Op } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import {
  GlialBinder,
  SessionDestination,
  type Fill,
  type InstanceStore,
  type OpBus,
  type SessionLike,
  type StoredOp,
  type StoreEngine,
  type WireOp,
} from "@owebeeone/glial-runtime";
import type { PayloadCodec } from "@owebeeone/glial-runtime/grip";
import type { BindingDecl, DomainAnchor, Shape, ZoneKind } from "@owebeeone/glade-decl";
import { manifestScope, surfaceDecl } from "../../grip-share/src/manifest.ts";
import { WORKSPACE_MANIFEST, stubGrant } from "./manifest";

const schema = loadSchema(gladeIr as never);
const appSchema = loadSchema(workspaceIr as never);

/** One activity-log entry — the declared `ChatLine` taut message. */
export interface ChatLine {
  ts: number;
  user: string;
  text: string;
}

/** taut codecs keyed by the manifest's surface `type`. Types absent here
 *  (e.g. "Text") use the JSON default — the same bytes as before the cutover. */
export const CODECS_BY_TYPE: Record<string, PayloadCodec> = {
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

// The share-space policy is manifest data; identity/doc come from the grant.
export const grant = stubGrant(user, doc);
export const scope = manifestScope(WORKSPACE_MANIFEST, grant);

// --- the one session + WS carrier ------------------------------------------

export const session = new Session(schema, origin);
export const client = new GladeClient(schema, origin, session);

/** The WS carrier as glial's `OpBus`: publish ships to the node; inbound node
 *  ops fan to every subscriber (each `SessionDestination` filters its route,
 *  echo-guarded by origin). */
class ClientBus implements OpBus {
  private handlers = new Set<(ops: WireOp[]) => void>();
  publish(ops: WireOp[]): void {
    client.sendOps(ops as unknown as Op[]);
  }
  onOps(handler: (ops: WireOp[]) => void): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }
  /** Inbound node ops (wired to `client.onOps` in glade.ts). */
  deliver(ops: Op[]): void {
    for (const h of [...this.handlers]) h(ops as unknown as WireOp[]);
  }
}
export const bus = new ClientBus();

// --- the browser store engine (glial rule 1, the GC-4 seam) -----------------
//
// Persistence first: every instance's op log rides sessionStorage, so a tab
// RELOAD restores the participant's OWN writes locally (remote state refills
// off the node replay; own-origin ops are echo-guarded out of it by design).
// sessionStorage (not localStorage) keeps the store per-tab, matching the
// per-tab origin/session model. Demo-grade: JSON+base64, no quota/eviction
// policy — the real engine (IndexedDB, retention-aware) is GC-4's.

const b64 = (b: Uint8Array) => btoa(String.fromCharCode(...b));
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

class BrowserInstanceStore implements InstanceStore {
  private ops: StoredOp[] = [];
  constructor(private readonly storageKey: string) {
    try {
      const raw = sessionStorage.getItem(storageKey);
      if (raw) {
        this.ops = (JSON.parse(raw) as Array<Record<string, unknown>>).map((o) => ({
          origin: o.origin as string,
          seq: o.seq as number,
          lamport: o.lamport as number,
          prev: o.prev == null ? null : unb64(o.prev as string),
          payload: unb64(o.payload as string),
        }));
      }
    } catch {
      this.ops = []; // a corrupt entry never wedges the app; the node refills
    }
  }
  append(op: StoredOp): void {
    // dedup by (origin, seq) — a re-delivered op is not stored twice.
    if (this.ops.some((o) => o.origin === op.origin && o.seq === op.seq)) return;
    this.ops.push(op);
    try {
      sessionStorage.setItem(
        this.storageKey,
        JSON.stringify(
          this.ops.map((o) => ({
            origin: o.origin,
            seq: o.seq,
            lamport: o.lamport,
            prev: o.prev == null ? null : b64(o.prev),
            payload: b64(o.payload),
          })),
        ),
      );
    } catch {
      // quota: keep serving from memory; persistence degrades, app does not.
    }
  }
  all(): StoredOp[] {
    return this.ops.slice();
  }
}

class BrowserStoreEngine implements StoreEngine {
  open(instanceKey: string): InstanceStore {
    return new BrowserInstanceStore(`glial:ops:${instanceKey}`);
  }
  drop(instanceKey: string): void {
    sessionStorage.removeItem(`glial:ops:${instanceKey}`);
  }
}

/** glial's instance registry — persistence first (browser store engine in the
 *  GC-4 seam), connectivity configured per mount via `destFor`. */
export const glial = new GlialBinder(new BrowserStoreEngine(), origin);

// --- manifest-derived declaration data per surface --------------------------

const ANCHOR: Record<string, DomainAnchor> = { doc: "document", account: "account" };

/** The app-static `BindingDecl` for a manifest surface (glade-decl vocabulary). */
export function declFor(gladeId: string): BindingDecl {
  const s = WORKSPACE_MANIFEST.surfaces[gladeId];
  if (!s) throw new Error(`manifest has no surface ${gladeId}`);
  return {
    glade_id: { id: gladeId },
    shape: s.shape as Shape,
    authority: "share",
    source: null,
    domain: ANCHOR[s.domain] ?? "deployment",
    zone: s.zone as ZoneKind,
    retention: { policy: "from_cursor", ttl_ms: null },
  };
}

/** The concrete fill for a surface: the decl's domain anchor filled with the
 *  open doc / the account owner; private zones keyed to the participant. */
export function fillFor(gladeId: string): Fill {
  const s = WORKSPACE_MANIFEST.surfaces[gladeId];
  if (!s) throw new Error(`manifest has no surface ${gladeId}`);
  const fill: Fill = { domain: s.domain === "doc" ? doc : user, zone: s.zone };
  if (s.zone === "private") fill.key = user;
  return fill;
}

/** Connectivity, config-as-data: the session-backed glade destination for a
 *  surface. The route's (share, key) comes from the SAME manifest scope the
 *  grip-share binder resolved — identical wire address, identical bytes. */
export function destFor(gladeId: string): (fill: Fill) => SessionDestination {
  const s = WORKSPACE_MANIFEST.surfaces[gladeId];
  if (!s) throw new Error(`manifest has no surface ${gladeId}`);
  const addr = scope.resolve(surfaceDecl(WORKSPACE_MANIFEST, gladeId));
  const route = { share: addr.share, gladeId, shape: s.shape, key: addr.key };
  return () => new SessionDestination(session as unknown as SessionLike, bus, route);
}

/** The surface's payload codec (undefined = the adapter's JSON default). */
export function codecFor(gladeId: string): PayloadCodec | undefined {
  const s = WORKSPACE_MANIFEST.surfaces[gladeId];
  return s ? CODECS_BY_TYPE[s.type] : undefined;
}
