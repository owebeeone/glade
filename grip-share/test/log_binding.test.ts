// Log-shaped binding through the glial path (coverage migrated from the
// binder-era log_binding tests with the GC-3 binder deletion): entries append
// as discrete ops and materialize as an ordered list; a peer replays the whole
// log cold, and a later peer resumes from a cursor to the same list.

import test from "node:test";
import assert from "node:assert/strict";

import type { Route } from "@owebeeone/glial-runtime";
import { LocalMesh, decl, localParticipant } from "./helpers.ts";

const D = decl("app:chat", "log");
const ROUTE: Route = { share: "app", gladeId: "app:chat", shape: "log", key: new Uint8Array() };

test("log entries append in order and replay cold + from a cursor", () => {
  const meshA = new LocalMesh();
  const A = localParticipant("a", meshA, D, { domain: "1" }, ROUTE);

  A.write("hello");
  A.write("world");
  assert.deepEqual(A.records(), ["hello", "world"]); // materialized locally
  const wire = meshA.captured;

  // COLD replay: a fresh peer folds the whole captured log
  const meshC = new LocalMesh();
  const C = localParticipant("c", meshC, D, { domain: "1" }, ROUTE);
  meshC.deliver(wire);
  assert.deepEqual(C.records(), ["hello", "world"]);

  // CURSOR replay: a peer that already has the first entry receives only the rest
  const meshD = new LocalMesh();
  const P = localParticipant("d", meshD, D, { domain: "1" }, ROUTE);
  meshD.deliver([wire[0]]); // up to cursor
  assert.deepEqual(P.records(), ["hello"]);
  meshD.deliver([wire[1]]); // resume from cursor
  assert.deepEqual(P.records(), ["hello", "world"]);
});

test("two log writers interleave deterministically", () => {
  const mesh = new LocalMesh();
  const FEED = decl("app:feed", "log");
  const route: Route = { share: "app", gladeId: "app:feed", shape: "log", key: new Uint8Array() };
  const A = localParticipant("a", mesh, FEED, { domain: "1" }, route);
  const B = localParticipant("b", mesh, FEED, { domain: "1" }, route);

  A.write("a1");
  B.write("b1");
  A.write("a2");

  // both sides converge to the same deterministic order
  assert.deepEqual(A.records(), B.records());
  assert.equal(A.records().length, 3);
});
