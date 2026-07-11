// The Gwz tab's wiring (GLP-0006 P1.S3) — grip-style, no React state hook for
// shared state. The gwz command surface is the composed glade-gwz supplier's
// EXCHANGE (ws-razel, gwz.ops): a request is a JSON envelope, the answer a
// {ok,exit,stdout,stderr} — failure is DATA (a disallowed verb comes back as
// {ok:false,error} — GladeSupplierModel §6), never a hang.
//
// Two run modes:
//   * runGwz  (stream:false) — client.exchange → the answer lands in GWZ_RESULT.
//   * streamGwz (stream:true) — the exchange answers {run_id,done:false}; we
//     subscribe (ws-razel, gwz.output, run_id) and point a glial LOG mount at
//     that run key (GWZ_RUN_ID drives its fill), so the run's output ops
//     converge live into GWZ_STREAM (mount via the typed `gwz.output` handle).

import { createAtomValueTap } from "@owebeeone/grip-react";
import { glialTap } from "@owebeeone/glial-runtime/grip";
import { SessionDestination, utf8, type Fill, type SessionLike } from "@owebeeone/glial-runtime";
import { defineManifest } from "@owebeeone/glial-runtime/manifest";
import { defineGrip, grok, main } from "./runtime";
import { bus, client, glial, session, user } from "./glial";

/** The gwz command exchange + long-op output surfaces (grazel-app.glade). */
export const GWZ_SHARE = "ws-razel";
export const GWZ_OPS_ID = "gwz.ops";
export const GWZ_OUTPUT_ID = "gwz.output";

/** The stage-1 read-only allow-list the supplier honors (exec.rs). The picker is
 *  limited to these; anything else the supplier refuses AS DATA. */
export const GWZ_VERBS = ["status", "ls", "diff"] as const;

/** The exchange answer (glade-gwz `GwzResponse`, JSON). Failure is data. */
export interface GwzResponse {
  ok: boolean;
  exit?: number;
  stdout?: string;
  stderr?: string;
  error?: string;
  run_id?: string;
  done?: boolean;
  attributed_to?: string;
}

/** One record on the gwz.output log for a streaming run (glade-gwz
 *  `GwzOutputRecord`, JSON). `stream` is "stdout" | "stderr" | "end". */
export interface GwzOutputRecord {
  run_id: string;
  seq: number;
  principal?: string;
  stream: string;
  line?: string;
  done?: boolean;
  exit?: number;
}

/** The typed `gwz.output` surface handle — the mount references this, never the
 *  raw glade-id string (P0.S5a compile wall). */
const gwzM = defineManifest({
  output: { id: GWZ_OUTPUT_ID, shape: "log", share: GWZ_SHARE, domain: "document", zone: "commons" },
});

// --- grips (shared state, no React state hook) ------------------------------

/** The selected verb (picker-limited to GWZ_VERBS). */
export const GWZ_VERB = defineGrip<string>("GwzVerb", GWZ_VERBS[0]);
export const GWZ_VERB_TAP = defineGrip<any>("GwzVerb.tap", undefined);
/** The last exchange answer (null until a run). */
export const GWZ_RESULT = defineGrip<GwzResponse | null>("GwzResult", null);
export const GWZ_RESULT_TAP = defineGrip<any>("GwzResult.tap", undefined);
/** The current streaming run id — drives the gwz.output mount's fill key. */
export const GWZ_RUN_ID = defineGrip<string>("GwzRunId", "");
export const GWZ_RUN_ID_TAP = defineGrip<any>("GwzRunId.tap", undefined);
/** The live streamed output records for the current run. */
export const GWZ_STREAM = defineGrip<GwzOutputRecord[]>("GwzStream", []);
export const GWZ_STREAM_TAP = defineGrip<any>("GwzStream.tap", undefined);

const enc = new TextEncoder();
const dec = new TextDecoder();

/** The wire destination for the gwz.output log, keyed by the current run id (the
 *  fill's key — set via GWZ_RUN_ID). The supplier appends keyed by run id, so a
 *  distinct run is a distinct key = a distinct instance/fold. */
function gwzOutputDest(fill: Fill): SessionDestination {
  const runId = String(fill.key ?? "");
  return new SessionDestination(session as unknown as SessionLike, bus, {
    share: GWZ_SHARE,
    gladeId: GWZ_OUTPUT_ID,
    shape: "log",
    key: utf8(runId),
  });
}

/** Register the Gwz tab's taps: the verb / result / run-id atoms + one glial LOG
 *  mount on gwz.output keyed by the run id (remounts as GWZ_RUN_ID changes). */
export function registerGwzTaps(): void {
  grok.registerTap(createAtomValueTap(GWZ_VERB, { initial: GWZ_VERBS[0], handleGrip: GWZ_VERB_TAP }) as never);
  grok.registerTap(createAtomValueTap<GwzResponse | null>(GWZ_RESULT, { initial: null, handleGrip: GWZ_RESULT_TAP }) as never);
  grok.registerTap(createAtomValueTap(GWZ_RUN_ID, { initial: "", handleGrip: GWZ_RUN_ID_TAP }) as never);
  grok.registerTap(
    glialTap<GwzOutputRecord[]>({
      binder: glial,
      decl: gwzM.output,
      grip: GWZ_STREAM,
      // fixed domain, run id as the key param — a run switch remounts the fold.
      fill: { domain: "gwz", key: { param: GWZ_RUN_ID } },
      handleGrip: GWZ_STREAM_TAP,
      gladeFor: gwzOutputDest,
    }) as never,
  );
}

// --- atom controllers (resolved once, like chat's postToGroup) --------------

let resultCtl: { set(v: GwzResponse | null): void } | undefined;
let runIdCtl: { set(v: string): void } | undefined;

function ctlFor<T>(tap: typeof GWZ_RESULT_TAP): { set(v: T): void } {
  const drip = grok.query(tap, main) as { get(): { set(v: T): void } | undefined };
  grok.flush();
  const ctl = drip.get();
  if (!ctl) throw new Error("gwz: atom controller not ready (tap unresolved)");
  return ctl;
}
function setResult(v: GwzResponse | null): void {
  resultCtl ??= ctlFor<GwzResponse | null>(GWZ_RESULT_TAP);
  resultCtl.set(v);
}
function setRunId(v: string): void {
  runIdCtl ??= ctlFor<string>(GWZ_RUN_ID_TAP);
  runIdCtl.set(v);
}

// --- the exchange ------------------------------------------------------------

/** The JSON request envelope the supplier decodes: {verb,args,stream,principal}.
 *  `principal = user` is the P0.S7 attribution stamp (stage-1: data, not gated). */
function envelope(verb: string, args: string[], stream: boolean): Uint8Array {
  return enc.encode(JSON.stringify({ verb, args, stream, principal: user }));
}

/** Turn the wire outcome into a GwzResponse. A wire ok:false (no provider /
 *  route error) is ALSO failure-as-data — surfaced honestly, never swallowed. */
function responseFrom(out: { ok: boolean; payload?: Uint8Array; error?: string }): GwzResponse {
  if (!out.ok) {
    return { ok: false, error: out.error ?? "no provider for gwz.ops (is the glade-gwz supplier attached?)" };
  }
  if (!out.payload) return { ok: false, error: "gwz: empty response payload" };
  try {
    return JSON.parse(dec.decode(out.payload)) as GwzResponse;
  } catch {
    return { ok: false, error: "gwz: undecodable response payload" };
  }
}

/** Run a verb (stream:false) and land the answer in GWZ_RESULT. `verb` may be a
 *  disallowed verb (the deny demo) — the supplier answers ok:false as data. */
export async function runGwz(verb: string, args: string[]): Promise<void> {
  const out = await client.exchange(GWZ_SHARE, GWZ_OPS_ID, envelope(verb, args, false));
  setResult(responseFrom(out));
}

/** Run a verb with stream:true: the exchange answers {run_id,done:false}; then
 *  subscribe the output surface for that run and point the glial mount at it so
 *  the output ops converge live into GWZ_STREAM. */
export async function streamGwz(verb: string, args: string[]): Promise<void> {
  const out = await client.exchange(GWZ_SHARE, GWZ_OPS_ID, envelope(verb, args, true));
  const resp = responseFrom(out);
  setResult(resp);
  if (resp.ok && resp.run_id) {
    // node interest + from-cursor history for this run, THEN mount the run key.
    await client.subscribe(GWZ_SHARE, GWZ_OUTPUT_ID, utf8(resp.run_id));
    setRunId(resp.run_id);
  }
}
