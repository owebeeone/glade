# The `<app>.glade` file format

An `<app>.glade` file declares what an application puts on glade: the surfaces
it shares, the services it answers, the access it starts with and the workspace
share it serves from. A glade node loads the file when it starts and registers
each declaration as an ordinary record. The file is data: nothing in it runs.

This page is about the line-oriented `glade-app` app-declaration file, the
`.glade` file a glade node loads; the brace-nested `.glade` files under
`dev-docs/examples/` in the glade-wz workspace are a different declaration
language, a design sketch that nothing implements.

The page describes the format as today's node accepts it.

## Words used here

- **share**: a replicated space, named by a token such as `ws-razel`.
- **surface**: one typed thing shared on a share, named by its **glade id**,
  such as `term.log`.
- **principal**: an identity that can be granted access, such as `owner`.

## An example

```text
# A small notes app.
glade-app v1
app notes

binding notes.list    value share commons latest
binding notes.edits   log   share commons from-cursor
binding notes.body    crdt  share commons from-cursor shape-profile=text_crdt
binding notes.cursor  value share private latest
binding notes.preview value share commons ttl ttl=10m

service notes notes.ops

seed owner ws-notes read.*,notes.*

workspace ws-notes notes
```

A node loads the files named by its `--app` flags when it starts with a profile,
for example `glade-node --profile local --app notes.glade`. The flag may
repeat, once per app: an app is declared by one file, and a node given two
files that name the same app refuses to start, naming both files. The shipped
[`apps/grazel-app.glade`](../apps/grazel-app.glade) is a longer example, with
comments.

## Grammar

```text
glade-app v1                # the header
app <name>                  # exactly once, before any other declaration
binding <glade_id> <shape> <authority> <zone> <retention> [ttl=<duration>] [shape-profile=<profile>]
service <name> <exchange-glade-id>
seed <principal> <share> <verb[,verb...]>
workspace <share> <name>
# a comment runs from `#` to the end of the line
```

- One declaration per line. Tokens are separated by spaces or tabs, and extra
  spaces are ignored, so columns may be aligned.
- `#` starts a comment wherever it appears, including after a declaration, and
  the comment runs to the end of the line. A token therefore cannot contain
  `#`. Blank lines are ignored.
- The first line that is not blank or a comment is the header: write
  `glade-app v1`. `glade-app v0` names the old language: a file headed with it
  still loads, with a warning that names the header to write, and no release is
  scheduled to stop loading it. The node reads both headers with one grammar,
  so a `glade-app v0` file may use every directive, token and key on this page,
  the keyword tail and `crdt` included. The header decides only how a zone or a
  retention that `glade-app v1` does not accept is treated (see
  [The zone](#the-zone) and [The retention](#the-retention)), and whether a
  `key=value` entry standing where either goes is refused (`v1`) or kept as
  written with a warning (`v0`).
- `app <name>` comes next, exactly once. Every `binding` and `service` record
  the file registers carries this name. An app is declared by one file: the
  node takes the file's `binding` lines as the app's whole set, so two files
  loaded at one start may not name the same app.
- Every directive takes exactly the tokens shown, and none has a default, so a
  line with a token missing or extra is refused. The one optional part of any
  line is a binding's keyword tail, the entries in brackets above (see
  [The keyword tail](#the-keyword-tail)).
- A glade id may appear once per file, across `binding` and `service` lines. A
  share may appear in one `workspace` line per file.
- A file that breaks a rule stops the node from starting; the message names the
  file and, where a line is at fault, its line number. The node reads every
  file before it writes anything, so a refused start leaves its store as it
  was. A file that loads can still carry warnings, which the node prints as
  `<file>: warning: line N: …` before it goes on.

**Spelling.** Multi-word tokens use the hyphen: `glade-app`, `from-cursor`,
`shape-profile`. Every multi-word token in the shipped app files does
(`ws-razel`, `glade-gyld`), and none uses an underscore. The two profile names
are the exception: `snapshot_delta` and `text_crdt` are the shape catalogue's
own names (GDL-041), written with the underscore exactly as shown.

### `binding`: a surface

`binding <glade_id> <shape> <authority> <zone> <retention>` declares a surface.
It has five tokens after `binding`, all required, and then may carry a keyword
tail.

| Token | What to write |
| --- | --- |
| `<glade_id>` | The surface's id: any single token. Dotted names such as `term.log` are a convention, not a rule. |
| `<shape>` | `value`: one value; concurrent writes resolve last-writer-wins. `log`: an append-only log, read from a cursor. `swmr`: one writer, many readers, as snapshots plus deltas. `crdt`: many writers whose concurrent edits merge rather than one overwriting another; the line must name its profile, `shape-profile=text_crdt`. The node refuses every other shape: `message`, `window` and `atom` are recognised and reserved, `stream` is recognised, and none of them can be bound; an exchange is declared with `service`, not `binding`. |
| `<authority>` | `share`: the share is the source of record. `external`: the share caches truth from an outside source. `external` is accepted with a warning: the file cannot name the source yet, so the binding registers and nothing acts on it. |
| `<zone>` | `commons` or `private`. See [The zone](#the-zone). |
| `<retention>` | `latest`, `from-cursor` or `ttl`. See [The retention](#the-retention). |

### The keyword tail

After its five tokens a binding line may carry `key=value` entries, in any
order, with no spaces around the `=`, each key at most once. There are two
keys.

| Key | What to write |
| --- | --- |
| `ttl=<duration>` | How long the surface's records last: a whole number above zero and exactly one unit, `ms`, `s`, `m`, `h` or `d`, with no space: `ttl=500ms`, `ttl=30s`, `ttl=10m`, `ttl=1h`, `ttl=7d`. One unit only, so write `ttl=90m`, not `ttl=1h30m`. It goes only on a line whose retention is `ttl`. |
| `shape-profile=<profile>` | The profile the shape runs with, spelled exactly: `snapshot_delta` (over `swmr`) or `text_crdt` (over `crdt`). A `crdt` line must carry `shape-profile=text_crdt`. A `swmr` line may carry `shape-profile=snapshot_delta`, or nothing, which means the plain engine. A `value` or `log` line takes none. |

For example:

```text
binding notes.preview value share commons ttl ttl=10m
binding notes.body    crdt  share commons from-cursor shape-profile=text_crdt
```

The node refuses, with the line number: an entry that is not `key=value`; a
key other than these two, which the message names; a key written twice; a
duration or a profile it does not know; `ttl=` on a line whose retention is not
`ttl`; a profile on a shape it does not fit; a `crdt` line without its profile;
and, in a file headed `glade-app v1`, a `key=value` entry standing where the
zone or the retention goes, which means one of the five tokens is missing. A
file headed `glade-app v0` keeps such an entry as the zone or the retention,
as it always did, with a warning that it belongs in the tail, after all five
tokens.

**You learn the tail here, not from the node.** A valid five-token line is
never refused, so the one message that shows the tail, the template the node
prints for a line with a token missing, never reaches an author whose line is
right. The tail is in the grammar above, in this section, and in the grammar
comment of the shipped app files.

**The node validates the tail keys `ttl=<duration>` and `shape-profile=<profile>`
and records neither: no record carries a duration or a profile yet.** The
registered record holds the five tokens only.

**Where a `crdt` mount gets its profile today.** A glial mount of a `crdt`
surface reads the profile declared for its glade id, a `ShapeProfileDecl`
record, ahead of the mount's own `MountConfig.crdtProfile`. No node registers a
`ShapeProfileDecl` yet, so today the profile comes from
`MountConfig.crdtProfile`: the application mounting the surface sets it to
`text_crdt`, or glial refuses the mount. The `shape-profile=text_crdt` a `crdt`
line must carry is checked by the node and reaches no mount.

### `service`: an exchange

`service <name> <exchange-glade-id>` declares an exchange: a directed
request/response surface, `<exchange-glade-id>`, answered by the service
`<name>`. Each request is answered by a single provider, the one attached on
the node that serves the request's share: an exchange never fans out.

### `seed`: a starting grant

`seed <principal> <share> <verb[,verb...]>` grants `<principal>` the listed
verbs on `<share>`. Verbs are separated by commas with no spaces, and a verb may
be a pattern such as `read.*`. At registration a seed becomes an ordinary grant
record, and a revocation always wins over it, even when the file is loaded
again. The node records grants but does not enforce them yet.

### `workspace`: the share this app serves from

`workspace <share> <name>` names the workspace share this app serves from and
gives it a display name. The node that loads the file registers itself as the
share's host and claims the share while it runs: this is the line that makes a
declared surface routable.

## The zone

The zone, token 4 of a binding line, says who converges on a surface: everyone
together, or each person on their own.

| Zone | Meaning |
| --- | --- |
| `commons` | Everyone who reaches the surface on its share converges on one copy. |
| `private` | Keyed to the principal: each person has a copy of their own. |

**Choosing.** Ask whether everyone should see the same data. If so, as for a
document body, a chat, a workspace tree or a build's output, write `commons`;
nearly every surface is `commons`. If each person should have their own, as for
a selection, a cursor or a draft, write `private`.

**Do not rely on `private` for confidentiality yet.** Only the grip-share binder
honours it. glial mounts do not produce the per-person key, so a surface
declared `private` and mounted through glial converges in the commons partition,
where everyone shares it.

**There is no default.** A binding line needs all five tokens after `binding`.
Leaving the zone out of a line without a tail leaves four, and the node refuses
the line with its line number and the template
`binding <glade_id> <shape> <authority> <zone> <retention> [ttl=<duration>] [shape-profile=<profile>]`.
On a line with a tail, the node refuses the tail entry that then stands where a
token goes. No zone is assumed.

**A mount does not override the zone you write.** The grip-share binder uses the
declared zone, and falls back to its manifest's zone only for a declaration that
has none. On the glial path the mount's zone fill never reaches the wire.

**Checking.** The node checks the zone in a file headed `glade-app v1`, the
header to write: any value other than `commons` or `private` is reported with
its line number and the two values. For now the report is a warning that says
a later node release refuses the line, and the node stores the zone as written
and starts. glade-node has had no release yet (its version is `0.0.0`): its
first release reports such a zone as a warning, and the release after that
refuses the file. A file headed `glade-app v0`, the old language, still loads
as it always did, with a warning that its header names the old language and
that you should write `glade-app v1`, plus a warning for each zone
`glade-app v1` does not accept. A `glade-app v0` file is never refused for its
zone.

## The retention

The retention, token 5 of a binding line, says how much of a surface's history
is kept.

| Retention | Meaning |
| --- | --- |
| `latest` | The surface keeps one value, and the last write wins. |
| `from-cursor` | The surface keeps its history, and a subscriber resumes from a position (a cursor) rather than from the newest value. |
| `ttl` | The surface's records expire after a duration. |

**Which to write** follows from the shape you wrote on the same line:

| Shape | Write | Because |
| --- | --- | --- |
| `value` | `latest` | A setting or a status, read at its current value. |
| `log` | `from-cursor` | An append log or an output stream: a reader resumes where it left off. |
| `swmr` | `from-cursor` | Single-writer state such as a workspace file tree is resumed, not last-write-wins. |
| `crdt` | Whatever matches how the surface is read | `from-cursor` if a subscriber resumes a history; `ttl` if its entries expire. Never `latest` by reflex: `latest` does not mean "the merged value". It keeps one value and lets the last write win, which is the opposite of a merge. |

No shipped app file declares a `crdt` surface yet, so there is no line to copy
the retention from: make the choice on purpose. The line also needs its profile,
`shape-profile=text_crdt`.

**`latest` is the one to be careful with.** It is legal on every shape, so
nothing warns you: on a `log` it declares that only the newest entry matters,
which turns an append log into a single value.

**`ttl` says how long in the tail.** It is the answer for a surface whose
entries should expire, such as a cache, on any shape. Write the duration as the
tail's `ttl=` key, for example
`binding notes.preview value share commons ttl ttl=10m`. A bare `ttl`, with no
`ttl=` entry, stays legal under either header and names no duration.

**`windowed` is not a retention.** A window, such as the last screenful of a
terminal, is a projection the application makes over a base shape, not
something the surface keeps. Where a file said `windowed`, write `from-cursor`
for the history and keep the window in the app.

**Spelling.** Write `from-cursor`, with a hyphen. `from_cursor`, with an
underscore, is how the contract (`glade-decl`) and the stored record spell it:
the node stores a file's `from-cursor` as `from_cursor`. Do not write the
underscore in a file.

**Retention is declarative.** Nothing enforces it yet: no node or client trims
or expires a surface by its retention today, whatever duration `ttl=` names.
Write the value that says how the surface is read.

**Checking.** The node checks the retention in a file headed `glade-app v1`,
the header to write: it must be one of the three values above, spelled as they
are here. Any other value is reported with its line number: `windowed` and
`from_cursor` are each told to write `from-cursor`, and any other value is told
the three. For now the report is a warning that says a later node release
refuses the line, and the node stores the retention as before (`from-cursor`
as `from_cursor`, anything else as written) and starts. As for the zone, the
first glade-node release reports such a retention as a warning and the release
after that refuses the file. A file headed `glade-app v0`, the old language,
still loads as it always did, with the header's warning and, for each
retention `glade-app v1` does not accept, a warning naming what to write and
`glade-app v1`, the version it changed in. A `glade-app v0` file is never
refused for its retention.

## Changing or deleting a line

A node reads its app files only when it starts, so an edit takes effect at the
next start. Registration is by difference: a declaration whose record the node
already holds is skipped, so loading an unchanged file registers nothing new.

An app is declared by one file. The node takes a file's `binding` lines as the
whole set its app declares, so two files naming one app would withdraw each
other's surfaces at every start. A node given two such files refuses to start,
before it writes anything, and the message names both files.

### `binding` lines

- **The newest declaration wins.** Binding records are folded per app and
  glade id, and the newest is that app's declaration of the surface; per glade
  id, the newest declaration still live across apps is the live one. So a
  changed line replaces the surface's declaration. The records it replaces stay
  in the store, and the node no longer treats them as declared. Two apps may
  declare one glade id: the node does not warn, and the newer live declaration
  stands.
- **A deleted line retracts its surface, for its own app only.** When a
  `binding` line is deleted from a file, the node records a retraction of the
  surface at its next start, but only of the declaration registered under the
  app that file names in its `app` line. A declaration made under another app
  is never touched, and if one is still live it becomes the surface's live
  declaration. Putting the line back declares the surface again.
- **A file that is not loaded retracts nothing.** A surface stays declared on a
  boot that leaves its file out, so a surface that a supplier will serve later
  stays declared. grazel, for instance, loads `gyld-app.glade` only when its
  gyld leg is on, and gyld's surfaces stay declared while the leg is off.
- **Renaming the `app` line starts another app.** The file's lines register
  under the new name, and the old name's declarations stay live, because no
  file the node loads names that app any more. So a line deleted later, under
  the new name, can bring back the old app's declaration of the same surface,
  as it was before the rename.
- **Retiring an app.** To withdraw every surface an app declared, start the
  node once with a file that names the app and has no `binding` lines: each of
  the app's declarations is retracted, and the file can then be left out.
  Retiring the old name this way is how to rename an app without leaving its
  declarations live.
- **The node stores the retention in the contract's spelling.** Whichever
  spelling the file uses, the stored record says `from_cursor`. So on a node
  whose records were written before it did this, each binding that says
  `from-cursor` is registered once more on the first start, now stored as
  `from_cursor`, and the fold makes that new record the live one.

### Other lines

`glade-app v1` gives `service` and `workspace` lines no retraction. That was
decided, not missed: the format's rule for deleted lines covers `binding` lines
only (ruling R9 of `dev-docs/glade/GladeDeclReconciliation.md` in the glade-wz
workspace), and whether a later format adds one is an open question.

- Deleting a `service` line retracts nothing: the exchange stays declared, and
  so stays routable.
- Deleting a `workspace` line: from its next start the node no longer serves
  the share, because at start it serves the shares its loaded files declare,
  so it stops claiming it. The registered entry that names the node as an
  eligible host of the share stays.
- Deleting a `seed` line does not withdraw the grant it made; revoking the
  grant does, and a revocation always wins over a seed.

## See also

- [`dev-docs/GladeGrazelAttachNotes.md`](../dev-docs/GladeGrazelAttachNotes.md):
  engineering notes on the parser and on registration.
- `dev-docs/glade/GladeDeclSurface.md` in the glade-wz workspace: the
  declaration vocabulary these tokens come from, including its `Domain` /
  `Zone` and `Retention` rows.
- [`node/src/appdecl.rs`](../node/src/appdecl.rs): the parser.
