import { type Grip } from "@owebeeone/grip-react";
import { glialTap } from "@owebeeone/glial-runtime/grip";
import { grok } from "./runtime";
import { SELECTION, SELECTION_TAP, NOTES, NOTES_TAP, ACTIVITY, ACTIVITY_TAP, STATUS, STATUS_TAP } from "./grips";
import { codecFor, declFor, destFor, fillFor, glial } from "./glial";
import type { ChatLine } from "./glade";

/** A cut-over surface: a glial mount consumed through the adapter tap. Decl,
 *  fill, destination and codec are all manifest-derived data (GC-3). */
const glialSurface = <T,>(gladeId: string, grip: Grip<T>, handleGrip?: Grip<any>) =>
  glialTap<T>({
    binder: glial,
    decl: declFor(gladeId),
    grip,
    fill: fillFor(gladeId),
    codec: codecFor(gladeId),
    handleGrip,
    gladeFor: destFor(gladeId),
  });

// A tap declares only *which* surface it provides (its glade id); the manifest
// owns the surface's domain/zone/shape (GladeZones.md, GladeManifest sketch) —
// that declaration is still the entire opt-in to sharing (GQ-5), now carried by
// the glial mount's manifest-derived decl/fill/route instead of a binder scope.
export function registerAllTaps(): void {
  // PRIVATE: my selection in this document — keyed to me, never shared.
  // CUT OVER (GC-3 3/4): a glial mount (the private-zone key rides the route).
  grok.registerTap(glialSurface("app:selection", SELECTION, SELECTION_TAP) as never);
  // COMMONS: the document's shared notes — everyone in this document.
  // CUT OVER (GC-3 2/4): a glial mount.
  grok.registerTap(glialSurface("app:notes", NOTES, NOTES_TAP) as never);
  // COMMONS (log): the document's activity feed. Entries append through the
  // glial controller (postActivity -> ACTIVITY_TAP.append), each its own op.
  // CUT OVER (GC-3 4/4): a glial mount with the taut ChatLine codec.
  grok.registerTap(glialSurface<ChatLine[]>("app:activity", ACTIVITY, ACTIVITY_TAP) as never);
  // ACCOUNT domain: my status follows me across documents (a different domain,
  // not this document) — proving a session is attached to several domains.
  // CUT OVER (GC-3 1/4): a glial mount; the grip-share binder no longer sees it.
  grok.registerTap(glialSurface("app:status", STATUS, STATUS_TAP) as never);
}
