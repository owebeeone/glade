# Gwz transactions — reliable multi-repo mutation (DRAFT proposal)

Status: DRAFT — design notes for the gwz-core disk layer, not yet ratified,
nothing here has landed. Owner seam: gwz-core (grazel/glade-gwz), which
`GladeDirectoryNotes.md` deliberately keeps EXTERNAL to the glade node — the
directory ceremony creates glade-side records only; disk materialization
(manifest + member clones + `workspace.lock`, trace D3) is gwz-core's. These
notes bind at that layer. The glade wire, the directory records, and the
exchange envelope (`GwzResponse` stays a JSON answer; failure stays DATA —
GladeSupplierModel §6) are all untouched.

Normative language per AGENTS.md: MUST / SHOULD / MAY.

## Problem

A gwz multi-repo merge is N ordinary git operations over N member clones. Two
distinct failure classes make it unrecoverable today:

1. **Within one repo**, git's merge state machine (`MERGE_HEAD`, `MERGE_MSG`,
   the index, the working tree) is ad-hoc files with no journal. A crash
   mid-merge leaves a state that can only be classified heuristically.
2. **Across repos**, git has no transaction at all. A crash after member 3 of
   7 has published leaves a torn workspace by construction, and no per-repo
   storage improvement can fix it.

The diagnosis worth keeping: git's stores are NOT uniformly unreliable.

| Store | Crash behavior |
| --- | --- |
| object store | Safe. Content-addressed, append-only, tmp+rename, idempotent writes; a dead operation leaves unreferenced objects that gc sweeps. Retry forever, no harm. |
| refs | Weak with the files backend (loose refs + `packed-refs` = sequenced renames). Git DOES have per-repo ref transactions (`git update-ref --stdin` `start`/`prepare`/`commit`) and the reftable backend (block-structured, transactional; `git refs migrate --ref-format=reftable`, git ≥ 2.45). |
| merge state / index / working tree | The bad part. No journal, no recovery function. |
| cross-repo | Nothing exists. This is gwz's to build. |

So the problem decomposes as (a) keep each per-repo step inside git's safe
stores, and (b) put the ONE atomic commit point in a real database at the
workspace layer. We do not need a modified git; we need to stop using its
unsafe stores during mutation and to add a coordinator.

## Design: workspace WAL + two-phase commit

One SQLite file per workspace is the "database": `.gwz/state.db`, beside the
manifest. SQLite is the smallest thing that is genuinely ACID, and it matches
the existing discipline — `workspace.lock` stays exactly as it is
(mutual exclusion, "filesystem lock is ground truth", WD §4 / the
`instance.lock` precedent in `GladeSystemDataSeamNotes.md`). The lock is
exclusion; the WAL is durability. They MUST NOT be conflated.

Schema (two tables):

```
ops(txid PRIMARY KEY, verb, principal, state, started_at, finished_at)
  -- state: preparing → prepared → committed → published | aborted
ref_edits(txid, member, ref, old_oid, new_oid)
```

Every mutating gwz verb becomes the same four phases:

1. **Prepare (per member, idempotent, invisible).** Compute the result and
   write objects only; park the result commit on a hidden ref
   `refs/gwz/pending/<txid>`. Nothing user-visible moves; the real branch,
   index, and working tree are untouched. Merge conflicts surface HERE, as
   data, before anything publishes. For merge specifically,
   `git merge-tree --write-tree` (git ≥ 2.38) computes the full merge and
   writes the tree without ever entering the `MERGE_HEAD`/index state
   machine — the prepare phase is side-effect-free by construction. Every
   step in this phase is an idempotent object/pending-ref write and MAY be
   retried blindly.
2. **Commit (the atomic point).** One SQLite transaction records the full
   old→new map in `ref_edits` and flips `ops.state` to `committed`. This is
   the ONLY durability point in the whole operation. Before it, the operation
   never happened; after it, it will happen.
3. **Publish (per member, idempotent CAS).** An atomic ref transaction per
   member (`git update-ref --stdin`: `start`/`prepare`/`commit`) moving each
   tracked ref from `old_oid` to `new_oid` (compare-and-swap on `old_oid`),
   then delete the pending ref. A ref already at `new_oid` is a no-op, so
   publish MAY be replayed. Flip `ops.state` to `published` when all members
   are done.
4. **Materialize (cache rebuild, outside the transaction).** Refresh working
   trees to the published oids. The working tree is a VIEW of committed refs,
   never transaction state (the jj stance); materialization is rerunnable at
   any time and a crash here loses nothing.

### Recovery

`recover()` MUST run unconditionally on every `workspace.lock` acquisition —
i.e. at the start of every gwz operation, not as a special repair verb — and
MUST be a pure function of (WAL, member repo states), no heuristics:

- `preparing`/`prepared` and not `committed` → roll BACK: delete
  `refs/gwz/pending/<txid>` in each member (orphaned objects are harmless),
  mark `aborted`.
- `committed` and not `published` → roll FORWARD: replay phase 3. Each CAS
  either applies, no-ops (already at `new_oid`), or fails as drift (below).

Because every step on both paths is an idempotent CAS or an idempotent
delete, recovery itself may crash and rerun.

### Drift is data, never corruption

The CAS is also the out-of-band detector. If anything moved a tracked ref
behind gwz's back (a direct `git commit`/`push` in a member, an IDE, a
script), the next publish or recovery finds `old_oid` stale and MUST fail as
a structured answer — "member X moved out-of-band from A to B" in
`GwzResponse` — never overwrite. A stale WAL entry meeting a foreign commit
fails cleanly; safety degrades to bookkeeping, never to a clobber.

A `reconcile` verb (or a flag on sync) makes that path first-class: scan
members, record observed out-of-band ref positions as an adopted operation in
the WAL (the `jj git import` precedent), and continue. With it, direct git
use of tracked branches is not forbidden, merely observed late.

## The boundary: what gwz owns vs. plain git

The transaction layer is a PUBLICATION boundary, not an editing interface —
the same shape as a git hosting server, which owns only the atomic ref update
at push time and wraps none of the CLI. Three tiers:

- **Untouched** — the whole read and local-work surface: log, diff, blame,
  bisect, grep, stash, index, commits, rebase of one's own branches,
  worktrees. None of it moves tracked refs; all of it stays plain git.
- **Mediated** — moving a MANIFEST-TRACKED ref of a member (each member's
  integration branch) publishes through the transaction. This is a handful of
  gwz verbs, not a mirror of git.
- **Forbidden** — a short blacklist: rewriting a tracked ref's history
  mid-transaction, hand-editing `refs/gwz/*`, deleting `.gwz/state.db`.

The boundary is only as real as the tracked-ref list. It MUST stay minimal —
one integration branch per member. A manifest that grows many tracked refs
per repo starts swallowing normal workflows; that pressure, not the
transaction machinery, is the thing to resist.

## What this buys beyond merge

- **Every cross-repo verb** (sync, branch, release-tag, cherry-pick trains)
  has the same torn-state failure mode and rides the same three phases with a
  different prepare.
- **A first-class workspace revision.** Each committed record is a coherent
  `(member, oid)` tuple set — reproducible builds pinned to a workspace tx,
  CI provenance, workspace-level bisect over consistent cross-repo states.
- **Workspace-wide undo.** Records carry the full old→new map; `gwz undo` /
  restore-to-tx is replay in reverse (the jj operation-log property).
- **Idempotent retries / safe concurrency at the exchange.** gwz is driven
  through the glade exchange, where requests retry and duplicate. Keyed by
  txid, a replayed request is a no-op; concurrent operations serialize on the
  lock or fail as data at the CAS — the "failure is DATA, never a hang"
  posture extended to disk.
- **A cross-repo invariant slot.** The commit point is where a
  workspace-wide check (version coherence, lockfile freshness) runs before
  anything publishes: atomic cross-repo changes land everywhere or nowhere.
- **Audit + status as a query.** The WAL is an attributed operation history
  (the `principal` column joins the P1 attribution seam); `gwz status` on an
  interrupted operation is a SELECT, not filesystem forensics.
- **A shippable log, later.** If nodes ever replicate or hand off workspace
  state, "stream the operation log" slots into the existing log/fold
  substrate; "rsync a maybe-consistent pile of clones" never will.

It buys nothing for single-repo, nothing-crashed workflows — plain git is
already fine there — and it adds the rule above: workspace-tracked refs move
through the transaction or show up as drift.

## Implementation sketch

New in gwz-core, one module + one refactor:

- `txn`: `begin(verb) → txid`, `stage(member, ref, old, new)`, `commit()`,
  `publish()`, `recover()`. SQLite plumbing included, on the order of
  500–800 lines. Publish drives `git update-ref --stdin` (or `gix` ref
  transactions — gitoxide is the Rust-native option if shelling out chafes).
- Each mutating verb splits: compute/object-write → prepare targeting
  `refs/gwz/pending/<txid>`; branch move → only ever inside `publish()`.
  Materialization strictly after publish.
- `recover()` wired into lock acquisition; CAS drift as a structured
  `GwzResponse` error; `reconcile` verb.

Unchanged: manifest format (pinning tracked-ref names explicitly is worth
doing), members as ordinary clones, `workspace.lock`, the exchange envelope,
the stage-1 read verbs (`status`/`ls`/`diff`), streaming output.

Staging — the smallest honest first step is WAL + `merge-tree` prepare + CAS
publish + recovery wrapped around the MERGE path only, files ref-backend, no
reconcile. That alone kills the unrecoverable-half-merged-workspace class.
Additive on the same tables, in rough order: the remaining mutating verbs;
`reconcile`; per-member reftable migration (hardens phase 3's atomicity;
needs git ≥ 2.45); workspace-revision queries; undo.

## Testing

Fault-injection is the gate, per the `GladeRustOrbitStrategy.md` discipline
("crash after each storage await and replay/recover"): a harness that kills
gwz after EVERY WAL write and every git child-process step, then runs
`recover()` and asserts the invariant — every member either at the old refs
or the new refs per the WAL's verdict, never mixed, `refs/gwz/pending/*`
empty, working trees rebuildable. Deterministic recovery is what makes this
matrix enumerable; drift injection (move a tracked ref mid-transaction)
asserts the CAS answers as data.

## Prior art (studied, not adopted wholesale)

- **reftable** (git core, from JGit/Gerrit): the per-repo transactional ref
  database; the drop-in hardening for phase 3.
- **jj (Jujutsu)** / `jj-lib`: the operation-log model — every mutation a
  recorded op over a ref snapshot, working copy as a transactional view,
  `git import` for adopting external changes, undo for free. The design to
  grow toward if workspace-wide undo and lock-free concurrency become
  first-class asks; `jj-lib` is embeddable Rust.
- **Fossil** (whole DVCS in one SQLite file), **Gerrit NoteDb**, hosted
  git-on-database deployments: existence proofs that git semantics over a
  real transactional store work; also why we do NOT need a bespoke object
  store — losing plain-git interop for every member clone would fix the
  layer that was not broken.

## Open questions

1. Shell out to git vs. `gix` for prepare/publish — decide on error-surface
   quality and process cost once the harness exists; the WAL schema does not
   care.
2. Where `reconcile`'s adopted state points on divergence (adopt, refuse, or
   record-and-branch) — stage-2, with the principal model.
3. Whether the workspace-revision id should surface in the directory records
   (a glade-side question, explicitly out of this seam's scope today).
