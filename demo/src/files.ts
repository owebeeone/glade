import type { InstanceEvent, FileWindow } from "@owebeeone/glial-runtime";
import { projectFileWindow } from "@owebeeone/glial-runtime";
import type { PayloadCodec } from "@owebeeone/glial-runtime/grip";
import { fromUtf8, utf8 } from "@owebeeone/glial-runtime";

/** First file profile: every snapshot/delta body is one complete UTF-8 image. */
export const FILE_CODEC: PayloadCodec = {
  encode: (value) => utf8(String(value ?? "")),
  decode: fromUtf8,
};

export const FILE_WINDOW_REQUEST = Object.freeze({ from: 0, length: 4096 });
export const EMPTY_FILE_WINDOW: FileWindow = Object.freeze({
  from: 0,
  length: 0,
  total: 0,
  bytes: new Uint8Array(),
  revision: "0:empty",
});

export function projectFileEvent(event: InstanceEvent): FileWindow {
  if (!event.swmr) throw new Error("ws.files projection requires an SWMR event");
  return projectFileWindow(event.swmr, FILE_WINDOW_REQUEST);
}
