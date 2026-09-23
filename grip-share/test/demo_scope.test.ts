// Characterization, glade-decl v1 under ruling R1(a) (reconciliation §4.7 row
// 13): `BindingDecl.domain` and `.zone` stay declaration-time hints that the
// binder resolves, so the demo's share-space policy still resolves
// `{domain: "account", zone: "private"}` to share "account:<user>" and key
// utf8("self:<user>") through `manifestScope`, with no edit to grip-share or
// the demo. The data is the demo's own `WORKSPACE_MANIFEST` and `stubGrant`.

import test from "node:test";
import assert from "node:assert/strict";

import { defineManifest } from "@owebeeone/glial-runtime/manifest";
import { manifestScope } from "../src/manifest.ts";
import { hex, utf8 } from "../../client-ts/src/bytes.ts";
import { WORKSPACE_MANIFEST, stubGrant } from "../../demo/src/manifest.ts";

// A typed handle declared account/private, as the demo declares its surfaces.
const H = defineManifest({
  accountPrivate: {
    id: "app:account-private", shape: "value", share: "account:{self}",
    domain: "account", zone: "private",
  },
});

test("the demo resolves {domain: account, zone: private} to share account:<user>, key self:<user>", () => {
  const s = H.accountPrivate;
  assert.equal(s.domain, "account");
  assert.equal(s.zone, "private");
  for (const user of ["alice", "bob"]) {
    const scope = manifestScope(WORKSPACE_MANIFEST, stubGrant(user, "7"));
    // The ShareDecl the demo's `resolveAddr` (demo/src/glial.ts) builds from a handle.
    const addr = scope.resolve({ gladeId: s.glade_id.id, shape: s.shape, domain: s.domain, zone: s.zone });
    assert.equal(addr.share, `account:${user}`);
    assert.equal(hex(addr.key), hex(utf8(`self:${user}`)));
  }
});
