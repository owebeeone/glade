/** Exact Glade SWMR op-payload adapter (`glade.swmr.adapter/v1`). */

export const SWMR_ADAPTER_VERSION = 1;
export type SwmrAction = "snapshot" | "delta" | "reset";

const TAGS: Readonly<Record<SwmrAction, number>> = Object.freeze({
  snapshot: 0,
  delta: 1,
  reset: 2,
});

export class SwmrActionError extends Error {
  readonly code = "GLADE_INVALID_SWMR_ACTION";

  constructor(message: string) {
    super(message);
    this.name = "SwmrActionError";
  }
}

export function encodeSwmrAction(action: SwmrAction, body: Uint8Array = new Uint8Array()): Uint8Array {
  const out = new Uint8Array(2 + body.length);
  out[0] = SWMR_ADAPTER_VERSION;
  out[1] = TAGS[action];
  out.set(body, 2);
  return out;
}

export function decodeSwmrAction(payload: Uint8Array): { action: SwmrAction; body: Uint8Array } {
  if (payload.length < 2) throw new SwmrActionError("SWMR action envelope is shorter than two bytes");
  if (payload[0] !== SWMR_ADAPTER_VERSION) {
    throw new SwmrActionError(`unsupported SWMR adapter version ${payload[0]}`);
  }
  const action = (["snapshot", "delta", "reset"] as const)[payload[1]];
  if (action === undefined) throw new SwmrActionError(`unsupported SWMR action tag ${payload[1]}`);
  return { action, body: payload.slice(2) };
}
