// The old binder.test.ts convergence scenarios, driven through the glial path
// (coverage migrated with the GC-3 binder deletion — same observable behavior,
// no grip-share binder). In-process: real @glade/client-ts sessions over a
// LocalMesh; assembly runs inside glial instances.

import test from "node:test";
import assert from "node:assert/strict";

import { GlialBinder, MemoryStoreEngine, type InstanceEvent, type Route } from "@owebeeone/glial-runtime";
import { JSON_PAYLOAD, LocalMesh, decl, jsonBytes, localParticipant } from "./helpers.ts";

const ROUTE: Route = { share: "app", gladeId: "app:count", shape: "value", key: new Uint8Array() };
const D = decl("app:count", "value");

// was: "two binders converge on an lww value; no echo loop"
test("two glial participants converge on an lww value; no echo loop", () => {
  const mesh = new LocalMesh();
  const A = localParticipant("a", mesh, D, { domain: "1" }, ROUTE);
  const B = localParticipant("b", mesh, D, { domain: "1" }, ROUTE);

  // A writes -> B converges
  A.write(5);
  assert.equal(B.value(), 5);

  // B writes (higher lamport) -> A converges to the lww winner
  B.write(9);
  assert.equal(A.value(), 9);
  assert.equal(B.value(), 9); // and B keeps its own value (no echo flip-flop)
});

// was: "consumer code is untouched: the tap's own set/get still work" — the
// observable: a binding works with ZERO connectivity (persistence first).
test("a mount with no glade destination still serves local writes", () => {
  const binder = new GlialBinder(new MemoryStoreEngine(), "a");
  const events: InstanceEvent[] = [];
  const m = binder.mount(decl("app:tab", "value"), { domain: "1" }, (e) => events.push(e)); // no config.glade
  assert.equal(m.instance.connected, false);

  m.instance.write(jsonBytes("calc"));
  const last = events[events.length - 1];
  assert.equal(JSON_PAYLOAD.decode(last.value!), "calc"); // local write folded + fanned
});

// was: "a late binder hydrates from peer state on bind"
test("a late glial participant hydrates from captured ops", () => {
  const meshA = new LocalMesh();
  const A = localParticipant("a", meshA, D, { domain: "1" }, ROUTE);
  A.write(7); // captured on meshA before B exists

  const meshB = new LocalMesh();
  const B = localParticipant("b", meshB, D, { domain: "1" }, ROUTE);
  meshB.deliver(meshA.captured); // the resume gap / node replay analog
  assert.equal(B.value(), 7);
});
