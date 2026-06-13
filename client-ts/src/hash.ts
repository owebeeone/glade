// Per-origin chain hash (GQ-9): op_hash = sha256(canonical_cbor(op)). The same
// canonical encoding the Rust node and Python reference use, so the hash agrees
// cross-language (reproduces taut/corpus/glade_hashes.json).

import * as codec from "./taut/codec.ts";
import type { SchemaIndex } from "./taut/schema.ts";
import { sha256 } from "./sha256.ts";

export function opHash(schema: SchemaIndex, op: Record<string, unknown>): Uint8Array {
  return sha256(codec.encode(schema, "Op", op));
}
