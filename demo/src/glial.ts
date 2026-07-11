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
  feedSession,
  GlialBinder,
  IndexedDbStoreEngine,
  SessionDestination,
  type Fill,
  type OpBus,
  type SessionLike,
  type WireOp,
} from "@owebeeone/glial-runtime";
import type { PayloadCodec } from "@owebeeone/glial-runtime/grip";
import type { Addr } from "../../grip-share/src/decl.ts";
import { manifestScope } from "../../grip-share/src/manifest.ts";
import { M, WORKSPACE_MANIFEST, stubGrant, type Surface } from "./manifest";

const schema = loadSchema(gladeIr as never);
const appSchema = loadSchema(workspaceIr as never);

/** One activity-log entry — the declared `ChatLine` taut message. `principal` is
 *  the NEW optional attribution field (GLP-0006 P1.S1, tag 4) — additive beside
 *  `user`, never a reinterpretation of it (absent on legacy lines). */
export interface ChatLine {
  ts: number;
  user: string;
  text: string;
  principal?: string;
}

/** The taut `ChatLine` codec — the one non-JSON payload. */
const CHATLINE_CODEC: PayloadCodec = {
  encode: (v) => tautCodec.encode(appSchema, "ChatLine", v as never),
  decode: (b) => tautCodec.decode(appSchema, "ChatLine", b),
};

/** Per-surface codec overrides, keyed by the typed handle. A surface absent
 *  here uses the adapter's JSON default — the same bytes as before the cutover. */
const CODEC_BY_SURFACE = new Map<Surface, PayloadCodec>([[M.activity, CHATLINE_CODEC]]);

// Identity is PER-TAB by ruling (Gianni, 2026-07-11): each tab is a distinct
// participant — the two-participant demo IS the product intent. The origin
// therefore lives in sessionStorage: a RELOAD keeps this tab's chain identity
// (the node resumes our log), while a second tab mints its own. Per-profile
// identity + a write-serializing store is explicitly NOT the demo's model.
// (Replaces the shared localStorage `glade-origin`, whose two-tabs-one-origin
// concurrent chain fork was the last GAP-9 residual.)
function stableOrigin(): string {
  const key = "glade-origin";
  let o = sessionStorage.getItem(key);
  if (!o) {
    o = Math.random().toString(36).slice(2, 8);
    sessionStorage.setItem(key, o);
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

// glial owns the carrier absorber natively (glial GAP-9): every inbound op
// reaches the session route-agnostically — truthful heads/resume vectors, and
// this tab's own chain resumes off the node replay after a reload.
feedSession(session as unknown as SessionLike, bus);

// --- the persistent store engine (glial rule 1, the GC-4 seam) --------------
//
// Persistence first: every instance's op log rides glial's IndexedDbStoreEngine
// (GC-4) — WHOLESALE wire records, so attachGlade hydration resumes this tab's
// session chain even offline-from-boot, and own state displays with no node.
// IndexedDB is per-PROFILE while identity is per-TAB (ruling above), so the
// database is keyed by this tab's origin: profile-wide storage never crosses
// identities. (Replaces the demo-grade sessionStorage BrowserStoreEngine,
// whose JSON projection dropped the wire fields hydration needs.)

/** glial's instance registry — persistence first (IndexedDB in the GC-4 seam),
 *  connectivity configured per mount via `destFor`. */
export const glial = new GlialBinder(await IndexedDbStoreEngine.open(`glial:${origin}`), origin);

// --- handle-derived declaration data per surface ----------------------------
//
// Each consumer passes a typed `Surface` handle (`M.notes`, …). A handle IS a
// `BindingDecl`, so the mount takes it directly (see taps.ts) — no `declFor`.
// The wire address still resolves through the SAME `manifestScope` + `Grant`.

/** The surface's wire `(share, key)`, resolved through the manifest scope +
 *  grant. The handle carries the domain anchor / zone; the scope owns the
 *  domain -> share / zone -> key policy — identical address, identical bytes. */
export function resolveAddr(s: Surface): Addr {
  return scope.resolve({ gladeId: s.glade_id.id, shape: s.shape, domain: s.domain, zone: s.zone });
}

/** The concrete fill for a surface: document-domain surfaces fill the open doc,
 *  account-domain surfaces fill the account owner; private zones key to you. */
export function fillFor(s: Surface): Fill {
  const fill: Fill = { domain: s.domain === "document" ? doc : user, zone: s.zone };
  if (s.zone === "private") fill.key = user;
  return fill;
}

/** Connectivity, config-as-data: the session-backed glade destination for a
 *  surface, addressed by the handle-resolved route. */
export function destFor(s: Surface): (fill: Fill) => SessionDestination {
  const addr = resolveAddr(s);
  const route = { share: addr.share, gladeId: s.glade_id.id, shape: s.shape, key: addr.key };
  return () => new SessionDestination(session as unknown as SessionLike, bus, route);
}

/** The surface's payload codec (undefined = the adapter's JSON default). */
export function codecFor(s: Surface): PayloadCodec | undefined {
  return CODEC_BY_SURFACE.get(s);
}
