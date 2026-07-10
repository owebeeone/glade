// Cross-language conformance (P2.S1/S2): the TS client reproduces taut's glade
// op-hash and fold oracles byte-for-byte — the browser folds with the same
// canonical results as the Rust node.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../src/taut/schema.ts";
import { opHash } from "../src/hash.ts";
import { foldLog, foldValue, isEquivocation, type FoldOp } from "../src/fold.ts";
import { hex, unhex } from "../src/bytes.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

test("op_hash reproduces the chain oracle (glade_hashes.json)", () => {
  const hashes = JSON.parse(readFileSync(join(corpus, "glade_hashes.json"), "utf8"));
  for (const v of hashes) {
    const o = v.op;
    const op = {
      share: o.share,
      glade_id: o.glade_id,
      key: unhex(o.key),
      origin: o.origin,
      seq: o.seq,
      prev: o.prev ? unhex(o.prev) : null,
      lamport: o.lamport,
      refs: [],
      shape: o.shape,
      payload: unhex(o.payload),
    };
    assert.equal(hex(opHash(schema, op)), v.hash, `hash mismatch for ${v.name}`);
  }
});

test("folds reproduce the fold oracle (taut-shape fold.v0.json)", () => {
  // Canonical fold semantics moved to taut-shape (Lane C P2, 2026-07-10);
  // taut's glade_folds.json remains as the frozen source it was re-homed from.
  const folds = JSON.parse(
    readFileSync(join(here, "..", "..", "..", "taut-shape", "corpus", "fold.v0.json"), "utf8"),
  ).vectors;
  for (const c of folds) {
    const ops: FoldOp[] = c.ops.map((o: any) => ({
      origin: o.origin,
      seq: o.seq,
      lamport: o.lamport,
      prev: o.prev ? unhex(o.prev) : null,
      payload: unhex(o.payload),
    }));
    if (c.fold === "value") {
      const r = foldValue(ops);
      assert.equal(r === null ? null : hex(r), c.expect, `value ${c.name}`);
    } else if (c.fold === "log") {
      assert.deepEqual(foldLog(ops).map(hex), c.expect, `log ${c.name}`);
    } else if (c.fold === "equiv") {
      assert.equal(isEquivocation(ops), c.expect, `equiv ${c.name}`);
    }
  }
});
