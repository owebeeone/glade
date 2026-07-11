import { type Grip, createAtomValueTap } from "@owebeeone/grip-react";
import { glialTap } from "@owebeeone/glial-runtime/grip";
import { grok } from "./runtime";
import {
  SELECTION, SELECTION_TAP, NOTES, NOTES_TAP, ACTIVITY, ACTIVITY_TAP,
  STATUS, STATUS_TAP, CURRENT_TAB, CURRENT_TAB_TAP,
} from "./grips";
import { codecFor, destFor, fillFor, glial } from "./glial";
import { M, type Surface } from "./manifest";
import type { ChatLine } from "./glade";

/** A cut-over surface: a glial mount consumed through the adapter tap. The typed
 *  `Surface` handle IS the `BindingDecl`; fill, destination and codec derive
 *  from it (GC-3). Consumers reference `M.notes`, never the string "app:notes". */
const glialSurface = <T,>(surface: Surface, grip: Grip<T>, handleGrip?: Grip<any>) =>
  glialTap<T>({
    binder: glial,
    decl: surface,
    grip,
    fill: fillFor(surface),
    codec: codecFor(surface),
    handleGrip,
    gladeFor: destFor(surface),
  });

// A tap declares only *which* surface it provides (its typed handle); the
// manifest owns the surface's domain/zone/shape (GladeZones.md, the typed
// manifest) — that declaration is still the entire opt-in to sharing (GQ-5),
// carried by the glial mount's handle-derived decl/fill/route.
export function registerAllTaps(): void {
  // PRIVATE: my selection in this document — keyed to me, never shared.
  grok.registerTap(glialSurface(M.selection, SELECTION, SELECTION_TAP) as never);
  // COMMONS: the document's shared notes — everyone in this document.
  grok.registerTap(glialSurface(M.notes, NOTES, NOTES_TAP) as never);
  // COMMONS (log): the document's activity feed. Entries append through the
  // glial controller (postActivity -> ACTIVITY_TAP.append), each its own op.
  grok.registerTap(glialSurface<ChatLine[]>(M.activity, ACTIVITY, ACTIVITY_TAP) as never);
  // ACCOUNT domain: my status follows me across documents (a different domain,
  // not this document) — proving a session is attached to several domains.
  grok.registerTap(glialSurface(M.status, STATUS, STATUS_TAP) as never);
  // Tab selection: shared state as a grip atom tap (no React state hook). The
  // tab bar switches it via CURRENT_TAB_TAP.set(id).
  grok.registerTap(
    createAtomValueTap(CURRENT_TAB, {
      initial: CURRENT_TAB.defaultValue ?? "workspace",
      handleGrip: CURRENT_TAB_TAP,
    }) as never,
  );
}
