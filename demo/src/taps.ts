import { createAtomValueTap } from "@owebeeone/grip-react";
import { grok } from "./runtime";
import { SELECTION, SELECTION_TAP, NOTES, NOTES_TAP, ACTIVITY } from "./grips";
import type { ChatLine } from "./glade";

// Each tap declares a glade `share` — that is the *entire* opt-in to sharing
// (GQ-5). The grip-share binder discovers them via grok.listSharedTaps().
export function registerAllTaps(): void {
  grok.registerTap(
    createAtomValueTap(SELECTION, {
      initial: "",
      handleGrip: SELECTION_TAP,
      share: { gladeId: "app:selection", shape: "value" },
    }),
  );
  grok.registerTap(
    createAtomValueTap(NOTES, {
      initial: "",
      handleGrip: NOTES_TAP,
      share: { gladeId: "app:notes", shape: "value" },
    }),
  );
  // ACTIVITY holds the materialized log list; entries are appended via the
  // binder (postActivity), not by setting a whole value.
  grok.registerTap(
    createAtomValueTap(ACTIVITY, {
      initial: [] as ChatLine[],
      share: { gladeId: "app:activity", shape: "log" },
    }),
  );
}
