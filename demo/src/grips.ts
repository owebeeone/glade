import { defineGrip } from "./runtime";
import type { ChatLine } from "./glade";

// Shared workspace state, across two domains + two zones (GladeZones.md):
//   SELECTION — doc domain, PRIVATE zone  (mine, this document)
//   NOTES     — doc domain, COMMONS zone  (everyone, this document)
//   ACTIVITY  — doc domain, COMMONS zone  (everyone, this document; a log)
//   STATUS    — ACCOUNT domain, commons   (mine, follows me across documents)
// The *_TAP handles let components write (handle.set(...)).
export const SELECTION = defineGrip<string>("Selection", "");
export const SELECTION_TAP = defineGrip<any>("Selection.tap", undefined);

export const NOTES = defineGrip<string>("Notes", "");
export const NOTES_TAP = defineGrip<any>("Notes.tap", undefined);

export const ACTIVITY = defineGrip<ChatLine[]>("Activity", []);
export const ACTIVITY_TAP = defineGrip<any>("Activity.tap", undefined);

export const STATUS = defineGrip<string>("Status", "");
export const STATUS_TAP = defineGrip<any>("Status.tap", undefined);
