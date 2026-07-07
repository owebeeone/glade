// Manifest-driven scope (GladeManifest): the share-space spec as data resolves
// the same (share, key) the hand-written scope did, with identity supplied by
// the grant — so two users' private zones can't collide.

import test from "node:test";
import assert from "node:assert/strict";

import { manifestScope, surfaceDecl, manifestCodecs, type Manifest, type Grant } from "../src/manifest.ts";
import { hex, utf8 } from "../../client-ts/src/bytes.ts";

const M: Manifest = {
  manifest: "test",
  version: 1,
  params: { self: { from: "identity" }, doc: { from: "session" } },
  domains: { doc: { share: "doc:{doc}" }, account: { share: "account:{self}" } },
  zones: { commons: { key: "" }, private: { key: "self:{self}" } },
  surfaces: {
    "app:notes": { domain: "doc", zone: "commons", shape: "value", type: "Text" },
    "app:selection": { domain: "doc", zone: "private", shape: "value", type: "Text" },
    "app:activity": { domain: "doc", zone: "commons", shape: "log", type: "ChatLine" },
    "app:status": { domain: "account", zone: "commons", shape: "value", type: "Text" },
  },
};

test("manifest scope resolves domain->share and zone->key from the grant", () => {
  const grant: Grant = { identity: { self: "alice" }, session: { doc: "7" } };
  const scope = manifestScope(M, grant);

  const notes = scope.resolve(surfaceDecl(M, "app:notes"));
  assert.equal(notes.share, "doc:7");
  assert.equal(notes.key.length, 0); // commons = empty key

  const sel = scope.resolve(surfaceDecl(M, "app:selection"));
  assert.equal(sel.share, "doc:7");
  assert.equal(hex(sel.key), hex(utf8("self:alice"))); // private = self-keyed

  const status = scope.resolve(surfaceDecl(M, "app:status"));
  assert.equal(status.share, "account:alice"); // account domain folds self into the share
  assert.equal(status.key.length, 0);
});

test("identity comes from the grant, so private keys can't collide across users", () => {
  const alice = manifestScope(M, { identity: { self: "alice" }, session: { doc: "7" } });
  const bob = manifestScope(M, { identity: { self: "bob" }, session: { doc: "7" } });
  const a = alice.resolve(surfaceDecl(M, "app:selection"));
  const b = bob.resolve(surfaceDecl(M, "app:selection"));
  assert.equal(a.share, b.share); // same document...
  assert.notEqual(hex(a.key), hex(b.key)); // ...different private zone keys
});

test("codecs are selected by surface type; unknown types fall through to default", () => {
  const chatLine = { encode: () => new Uint8Array(), decode: () => null };
  const codecs = manifestCodecs(M, { ChatLine: chatLine });
  assert.equal(codecs.get("app:activity"), chatLine); // ChatLine surface -> the codec
  assert.equal(codecs.has("app:notes"), false); // Text not registered -> binder default (JSON)
});
