# Glade Zones — domain / zone / surface

Status: implemented + verified (2026-06-14). Vocabulary settled in a design
conversation; keyed routing now lives in the node, client, binder, and demo —
commons-vs-private proven end-to-end through the rust node and the browser folds
(`grip-share` `zones` test + the two-participant demo).

Purpose: name how shared state is partitioned — *which* replicated world,
*who* converges within it, and *what* typed thing is shared. This replaces the
overloaded words "session" and "scope", which each carried several ideas.

## The knot we untied

"Session" was doing two unrelated jobs at once:

1. **which context-instance** you are in — `?doc=1` vs `?doc=2`, like two
   different Google Docs; and
2. **how widely a surface is shared** — your cursor vs the document body vs your
   app settings differ in *who converges*, independent of which document.

These are orthogonal. Splitting them gives three crisp terms.

## The model

| Term | Meaning | Wire field |
| --- | --- | --- |
| **domain** | The replicated world. Anchored to an *account* (your personal domain) or a *document/workspace* (a shared domain). | `share` |
| **zone** | The converging partition *within* a domain: `commons` (everyone in the domain) or `private` (keyed to a self). | `key` |
| **surface** | The typed shared thing within a `(domain, zone)` — a declared Definition (glade id + shape + payload type). | `glade_id` |

A participant's session is attached to **several domains at once** — always its
account domain, plus each open document domain.

The two axes — *which domain* and *which zone* — name the cases that "session"
muddled:

| surface | domain | zone | reads as |
| --- | --- | --- | --- |
| app settings | your **account** | (yours alone) | yours, in **every** document |
| doc body / chat / notes | the **document** | **commons** | everyone, **this** document |
| my selection / cursor | the **document** | **private** | yours, **this** document |
| global notice | the **deployment** | **commons** | everyone, everywhere |

"Universal vs document" was the **domain** choice; "shared vs mine" was the
**zone** choice. Never one thing.

## Privacy by construction vs. sharing by grant

The two zone kinds are protected by *different mechanisms*, and they do not
interfere:

- **private** — private by *keying*, not permission. The key includes the self
  (`self:alice`); only that self's sessions ever produce or subscribe to that
  key, so no one else receives it — even inside a shared document domain. No
  ACL, no capability, no setup. Joining a shared doc auto-grants you *your own*
  private zone (`self:me`), with no collision and no leak.
- **commons** — the only thing access-control gates. "Share this document with
  you" = "you may join this domain's **commons** zone." That grant is the whole
  sharing act.

So: **sharing is a grant; privacy is a key.** Many domains, many people, each
with a private layer over a shared commons — the Google-Docs shape.

Honest caveat: private-by-keying is *routing*-private. It holds while the node
honoring the keys is trusted (the gryth node is yours). Untrusted relays would
need encryption on the private key's traffic — a clean, separable step.

## Where (deferred) security actually acts

This bounds the security model precisely:

- **grants** gate **commons-zone joins** (a capability per `(domain, commons)`);
- **encryption** (if ever) protects **private zones** over untrusted hops;
- nothing in between — `private` needs no grant, `commons` needs no encryption
  on a trusted node.

(See `GladeGrythSecurityModelAnalysisPrompt.md`; this is the granularity its
grants operate at.)

## Wire mapping — already frozen

`domain + zone + surface` is a *renaming* of the address tuple every op already
carries:

```
domain  -> share      (the replicated world)
zone    -> key        (""=commons, "self:alice"=private)
surface -> glade_id   (the typed Definition)
```

The node now routes by `(share, glade_id, key)`; domain->share, zone->key,
surface->glade_id. No new wire. Enabling it **refined D8**: `key` became part of
the **chain axis** (alongside `glade_id`), because a private zone must be
filterable from what a peer receives and a hash chain can't be filtered and
still verify — so each zone is its own contiguous chain. The on-disk journal
stays per-`(share, origin)`; ops regroup into chains by their own fields on load.

## Zone keys (the partition axes)

A zone's `key` is a tuple of the *included* partition axes:

- **self** (the person) → `private` (keyed `self:<userId>`);
- *(none)* → `commons` (empty key);
- future axes compose the same way: **device**, **cohort** (a group/team),
  a transient **tab**. A surface declares which axes it weaves into its key.

## Typed manifest (GLP-0006 P0.S5b, 2026-07-12)

The demo's surface table is now typed handles (`defineManifest` → `M.notes` …,
the declared-surface compile wall). The grip-share `manifestScope`/`Grant` policy
is unchanged, with ONE faithful edit: `WORKSPACE_MANIFEST.domains` is keyed by the
canonical `DomainAnchor` the handle carries (`document`/`account`) instead of the
ad-hoc labels (`doc`/`account`). The share-template VALUES (`doc:{doc}`,
`account:{self}`) and the zone→key mapping are identical, so the wire address /
bytes are unchanged; the rekey just lets the scope resolve straight off the typed
handle's `domain`, retiring the old `ANCHOR` name→anchor translation table. The
surfaces moved out of `WORKSPACE_MANIFEST` (now empty there) into `M`.

## Open

- The full axis vocabulary beyond `self` (device, cohort, …).
- The account/universal domain's exact shape (one domain per principal?).
- Domain anchoring: how `?doc=N` resolves to a domain id, and how a session
  declares which domains it joins.
- Whether `domain`/`zone` replace `share`/`key` as the wire field names, or stay
  the declaration-level vocabulary over the existing wire.
