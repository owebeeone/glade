// Toolchain core through the glial path (coverage migrated from the binder-era
// node_integration tests with the GC-3 binder deletion): a browser-shaped
// participant — ONE session + ONE ws client + ONE glial binder carrying
// SEVERAL mounted bindings — converges through the REAL rust glade-node, value
// (lww) and log shapes, commons and private zones.
// Requires the node binary: cargo build --bin glade-node in ../../node.

import test from "node:test";
import assert from "node:assert/strict";

import { Session } from "../../client-ts/src/session.ts";
import { GladeClient } from "../../client-ts/src/client.ts";
import { utf8 } from "../../client-ts/src/bytes.ts";
import { GlialBinder, MemoryStoreEngine, type Route } from "@owebeeone/glial-runtime";
import { ClientBus, JSON_PAYLOAD, decl, mountView, schema, startNode, until } from "./helpers.ts";

/** A browser-shaped participant: several mounts on one session/client/bus. */
async function makeBrowser(origin: string, url: string, routes: Record<string, Route>) {
  const session = new Session(schema, origin);
  const client = new GladeClient(schema, origin, session);
  const bus = new ClientBus();
  bus.client = client;
  client.onOps = (ops) => {
    session.applyRemote(ops);
    bus.deliver(ops);
  };
  const binder = new GlialBinder(new MemoryStoreEngine(), origin);
  const views = Object.fromEntries(
    Object.entries(routes).map(([name, route]) => [
      name,
      mountView(binder, session, bus, decl(route.gladeId, route.shape as "value" | "log"), { domain: "1" }, route, JSON_PAYLOAD),
    ]),
  );
  await client.connect(url);
  for (const r of Object.values(routes)) {
    await client.subscribe(r.share, r.gladeId, r.key.length ? r.key : undefined);
  }
  return { client, views };
}

test("workspace surfaces converge through the rust node (lww + log)", async () => {
  const { port, child } = await startNode("grip-share");
  const url = `ws://127.0.0.1:${port}`;
  const routes: Record<string, Route> = {
    selection: { share: "app", gladeId: "app:selection", shape: "value", key: new Uint8Array() },
    notes: { share: "app", gladeId: "app:notes", shape: "value", key: new Uint8Array() },
    activity: { share: "app", gladeId: "app:activity", shape: "log", key: new Uint8Array() },
  };
  try {
    const A = await makeBrowser("a", url, routes);
    const B = await makeBrowser("b", url, routes);

    // lww: A picks a selection -> B sees it
    A.views.selection.write("src/main.rs");
    await until(() => B.views.selection.value() === "src/main.rs");

    // lww: B edits notes -> A sees it
    B.views.notes.write("review the resolver");
    await until(() => A.views.notes.value() === "review the resolver");

    // log: A logs activity, then B logs activity -> both converge in order
    A.views.activity.write("A opened src/main.rs");
    await until(() => B.views.activity.records().length === 1);
    B.views.activity.write("B added a note");
    await until(() => A.views.activity.records().length === 2);

    assert.deepEqual(A.views.activity.records(), B.views.activity.records());
    assert.equal(A.views.activity.records().length, 2);

    A.client.close();
    B.client.close();
  } finally {
    child.kill();
  }
});

test("zones: commons converges, private stays per-user", async () => {
  const { port, child } = await startNode("grip-share-zones");
  const url = `ws://127.0.0.1:${port}`;
  // one participant = notes (commons) + selection (private, self-keyed).
  const zonedRoutes = (user: string): Record<string, Route> => ({
    selection: { share: "doc:1", gladeId: "app:selection", shape: "value", key: utf8(`self:${user}`) },
    notes: { share: "doc:1", gladeId: "app:notes", shape: "value", key: new Uint8Array() },
  });
  try {
    const alice = await makeBrowser("alice", url, zonedRoutes("alice"));
    const bob = await makeBrowser("bob", url, zonedRoutes("bob"));

    // commons zone (empty key): alice's note reaches bob
    alice.views.notes.write("shared agenda");
    await until(() => bob.views.notes.value() === "shared agenda");

    // private zone (self-keyed): each picks a selection; neither crosses
    alice.views.selection.write("src/main.rs");
    bob.views.selection.write("Cargo.toml");
    await until(() => bob.views.selection.value() === "Cargo.toml");
    await until(() => alice.views.selection.value() === "src/main.rs");
    // let any (erroneous) cross-delivery arrive, then assert isolation held
    await new Promise((r) => setTimeout(r, 120));
    assert.equal(alice.views.selection.value(), "src/main.rs"); // never bob's
    assert.equal(bob.views.selection.value(), "Cargo.toml"); // never alice's
    assert.equal(alice.views.notes.value(), "shared agenda"); // commons still shared

    alice.client.close();
    bob.client.close();
  } finally {
    child.kill();
  }
});
