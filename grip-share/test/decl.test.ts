// The surviving declaration plumbing (post GC-3): the share-declaration
// vocabulary and the manifest input. (was part of binder.test.ts)

import test from "node:test";
import assert from "node:assert/strict";

import { collectGladeIds, type GrokLike, type SharableTap } from "../src/decl.ts";

function declared(gladeId: string): SharableTap {
  return { share: { gladeId, shape: "value" } };
}
function grokOf(...taps: SharableTap[]): GrokLike {
  return { listSharedTaps: () => taps };
}

test("collectGladeIds yields stable, sorted ids (GQ-6 manifest input)", () => {
  const grok = grokOf(
    declared("app:count"),
    declared("app:tab"),
    declared("app:count"), // duplicate id -> deduped
  );
  assert.deepEqual(collectGladeIds(grok), ["app:count", "app:tab"]);
});
