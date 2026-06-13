// Log-shaped binding (P3.S3): entries append as discrete ops and materialize as
// an ordered list; a peer replays the whole log cold, and a later peer resumes
// from a cursor (incremental ops) to the same list.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { loadSchema } from "../../client-ts/src/taut/schema.ts";
import { Session, type Op } from "../../client-ts/src/session.ts";
import { GripShareBinder, type GrokLike, type SharableTap } from "../src/binder.ts";

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, "..", "..", "..", "taut", "corpus");
const schema = loadSchema(JSON.parse(readFileSync(join(corpus, "glade.ir.json"), "utf8")));

/** A log tap double: holds the materialized ordered list; appends go via the binder. */
function fakeLog(gladeId: string) {
  let list: unknown[] = [];
  return {
    share: { gladeId, shape: "log" },
    get: () => list,
    applyShareValue: (v: unknown) => {
      list = v as unknown[];
    },
  };
}
function grokOf(...taps: SharableTap[]): GrokLike {
  return { listSharedTaps: () => taps };
}

test("log entries append in order and replay cold + from a cursor", () => {
  const logA = fakeLog("app:chat");
  const binderA = new GripShareBinder(grokOf(logA), new Session(schema, "a"));
  const wire: Op[] = [];
  binderA.onLocalOps = (ops) => wire.push(...ops);
  binderA.bind();

  binderA.appendLog("app:chat", "hello");
  binderA.appendLog("app:chat", "world");
  assert.deepEqual(logA.get(), ["hello", "world"]); // materialized locally

  // COLD replay: a fresh peer folds the whole captured log
  const logCold = fakeLog("app:chat");
  const binderCold = new GripShareBinder(grokOf(logCold), new Session(schema, "c"));
  binderCold.bind();
  binderCold.applyRemote(wire);
  assert.deepEqual(logCold.get(), ["hello", "world"]);

  // CURSOR replay: a peer that already has the first entry receives only the rest
  const logCursor = fakeLog("app:chat");
  const binderCursor = new GripShareBinder(grokOf(logCursor), new Session(schema, "d"));
  binderCursor.bind();
  binderCursor.applyRemote([wire[0]]); // up to cursor
  assert.deepEqual(logCursor.get(), ["hello"]);
  binderCursor.applyRemote([wire[1]]); // resume from cursor
  assert.deepEqual(logCursor.get(), ["hello", "world"]);
});

test("two log writers interleave deterministically", () => {
  const logA = fakeLog("app:feed");
  const logB = fakeLog("app:feed");
  const binderA = new GripShareBinder(grokOf(logA), new Session(schema, "a"));
  const binderB = new GripShareBinder(grokOf(logB), new Session(schema, "b"));
  binderA.onLocalOps = (ops) => binderB.applyRemote(ops);
  binderB.onLocalOps = (ops) => binderA.applyRemote(ops);
  binderA.bind();
  binderB.bind();

  binderA.appendLog("app:feed", "a1");
  binderB.appendLog("app:feed", "b1");
  binderA.appendLog("app:feed", "a2");

  // both sides converge to the same deterministic order
  assert.deepEqual(logA.get(), logB.get());
  assert.equal((logA.get() as unknown[]).length, 3);
});
