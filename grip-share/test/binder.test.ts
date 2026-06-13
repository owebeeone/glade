// Binder convergence + echo guard (P3.S2). Two binders over a loopback
// converge their bound taps via the real glade Session/folds. Tap doubles
// implement exactly the share hooks AtomValueTap has (P3.S1), so this exercises
// the real binding contract without dragging grip-core through --strip-types.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import { Session } from "../../client-ts/src/session.ts";
import { GripShareBinder, collectGladeIds, type GrokLike, type SharableTap } from "../src/binder.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

/** A tap double with the same share hooks AtomValueTap implements. */
function fakeAtom(gladeId: string, shape: string, initial: unknown) {
  let value = initial;
  const listeners = new Set<() => void>();
  const fire = () => listeners.forEach((l) => l());
  return {
    share: { gladeId, shape },
    get: () => value,
    set: (v: unknown) => {
      if (v !== value) {
        value = v;
        fire();
      }
    },
    getShareValue: () => value,
    applyShareValue: (v: unknown) => {
      if (JSON.stringify(v) !== JSON.stringify(value)) {
        value = v;
        fire();
      }
    },
    subscribeShare: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}

function grokOf(...taps: SharableTap[]): GrokLike {
  return { listSharedTaps: () => taps };
}

/** Wire two binders as in-process peers (each forwards local ops to the other). */
function loopback(a: GripShareBinder, b: GripShareBinder) {
  a.onLocalOps = (ops) => b.applyRemote(ops);
  b.onLocalOps = (ops) => a.applyRemote(ops);
}

test("two binders converge on an lww value; no echo loop", () => {
  const tapA = fakeAtom("app:count", "value", 0);
  const tapB = fakeAtom("app:count", "value", 0);
  const binderA = new GripShareBinder(grokOf(tapA), new Session(schema, "a"));
  const binderB = new GripShareBinder(grokOf(tapB), new Session(schema, "b"));
  loopback(binderA, binderB);
  binderA.bind();
  binderB.bind();

  // A writes -> B converges
  tapA.set(5);
  assert.equal(tapB.get(), 5);

  // B writes (higher lamport) -> A converges to the lww winner
  tapB.set(9);
  assert.equal(tapA.get(), 9);
  assert.equal(tapB.get(), 9); // and B keeps its own value (no echo flip-flop)
});

test("consumer code is untouched: the tap's own set/get still work", () => {
  // binding is additive — a bound tap behaves like a normal atom locally
  const tap = fakeAtom("app:tab", "value", "clock");
  const binder = new GripShareBinder(grokOf(tap), new Session(schema, "a"));
  binder.bind();
  tap.set("calc");
  assert.equal(tap.get(), "calc"); // local set still works, no binder interference
});

test("collectGladeIds yields stable, sorted ids (GQ-6 manifest input)", () => {
  const grok = grokOf(
    fakeAtom("app:count", "value", 0),
    fakeAtom("app:tab", "value", "clock"),
    fakeAtom("app:count", "value", 0), // duplicate id -> deduped
  );
  assert.deepEqual(collectGladeIds(grok), ["app:count", "app:tab"]);
});

test("a late binder hydrates from peer state on bind", () => {
  const tapA = fakeAtom("app:count", "value", 0);
  const binderA = new GripShareBinder(grokOf(tapA), new Session(schema, "a"));
  const sessionB = new Session(schema, "b");
  // A writes before B exists
  const captured: any[] = [];
  binderA.onLocalOps = (ops) => captured.push(...ops);
  binderA.bind();
  tapA.set(7);

  // B joins later, replays captured ops, then binds -> hydrates to 7
  const tapB = fakeAtom("app:count", "value", 0);
  const binderB = new GripShareBinder(grokOf(tapB), sessionB);
  binderB.applyRemote(captured); // resume gap
  binderB.bind();
  assert.equal(tapB.get(), 7);
});
