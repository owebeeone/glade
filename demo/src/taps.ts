import { createAtomValueTap } from "@owebeeone/grip-react";
import { surfaceDecl } from "../../grip-share/src/manifest.ts";
import { grok } from "./runtime";
import { SELECTION, SELECTION_TAP, NOTES, NOTES_TAP, ACTIVITY, STATUS, STATUS_TAP } from "./grips";
import { WORKSPACE_MANIFEST } from "./manifest";
import type { ChatLine } from "./glade";

// A tap declares only *which* surface it provides (its glade id); the manifest
// owns the surface's domain/zone/shape (GladeZones.md, GladeManifest sketch).
// That declaration is the entire opt-in to sharing (GQ-5); the binder's
// (manifest-derived) scope maps domain+zone to the wire (share, key).
// (`as never`: grip-core's dist now types ShareDecl.shape/domain as the
// glade-decl unions while grip-share's structural ShareDecl is stringly — a
// pre-existing drift; this whole helper dies with the GC-3 cutover.)
const share = (gladeId: string) => surfaceDecl(WORKSPACE_MANIFEST, gladeId) as never;

export function registerAllTaps(): void {
  // PRIVATE: my selection in this document — keyed to me, never shared.
  grok.registerTap(
    createAtomValueTap(SELECTION, { initial: "", handleGrip: SELECTION_TAP, share: share("app:selection") }),
  );
  // COMMONS: the document's shared notes — everyone in this document.
  grok.registerTap(
    createAtomValueTap(NOTES, { initial: "", handleGrip: NOTES_TAP, share: share("app:notes") }),
  );
  // COMMONS (log): the document's activity feed. Entries are appended via the
  // binder (postActivity), not by setting a whole value.
  grok.registerTap(
    createAtomValueTap(ACTIVITY, { initial: [] as ChatLine[], share: share("app:activity") }),
  );
  // ACCOUNT domain: my status follows me across documents (a different domain,
  // not this document) — proving a session is attached to several domains.
  grok.registerTap(
    createAtomValueTap(STATUS, { initial: "", handleGrip: STATUS_TAP, share: share("app:status") }),
  );
}
