import { defineGrip } from "./runtime";
import type { ChatLine } from "./glade";

// Shared workspace state. SELECTION + NOTES are lww values; ACTIVITY is a log.
// The *_TAP handles let components write (handle.set(...)).
export const SELECTION = defineGrip<string>("Selection", "");
export const SELECTION_TAP = defineGrip<any>("Selection.tap", undefined);

export const NOTES = defineGrip<string>("Notes", "");
export const NOTES_TAP = defineGrip<any>("Notes.tap", undefined);

export const ACTIVITY = defineGrip<ChatLine[]>("Activity", []);
