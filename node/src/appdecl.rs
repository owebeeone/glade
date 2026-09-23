//! The `<app>.glade` loader (GDL-037, Lane R step 4) — an application's
//! declaration file, LOADED as runtime data, never a compiler front-end.
//!
//! The file is the legible app surface: its BindingDecls + ServiceDefinitions
//! register as ordinary home-share records, and its ACL seeds COMPILE TO
//! CapabilityGrant records — all appended under the REGISTRANT's chain, byte-
//! identical to what dynamic configuration writes. There is no second,
//! privileged "install" path: an app only ever CONTRIBUTES records, and base
//! glade folds them without knowing any app (grazel is just the first).
//!
//! Format (the smallest faithful serialization — line-oriented text, zero
//! deps, diff-friendly; see `apps/grazel-app.glade` and
//! `dev-docs/GladeGrazelAttachNotes.md` for the choice):
//!
//! ```text
//! glade-app v1                 # version header, first declaration line (v0: the old language, warned)
//! app <name>                   # exactly once, before any declaration
//! binding <glade_id> <shape> <authority> <zone> <retention> [ttl=<duration>] [shape-profile=<profile>]
//! service <name> <exchange-glade-id>
//! seed <principal> <share> <verb[,verb...]>
//! workspace <share> <name>     # a workspace share this app serves from
//! ```
//!
//! A binding line has five positional tokens, then an optional keyword tail
//! (R11(a)): `key=value` entries in any order, each key at most once — see
//! the `tail` module for its rules. The retention is stored in the contract's
//! spelling: a file writes `from-cursor`, the record holds `from_cursor`
//! (R9(b2)).
//!
//! The header names the file's language (R10(a)): `glade-app v0` or
//! `glade-app v1`, recorded as [`AppDecl::version`]; any other first
//! declaration line is refused with its line number. `v1` is the validated
//! grammar: a binding's zone must be `commons` or `private`, and its retention
//! `latest`, `from-cursor` or `ttl` in the file's spelling; any other token is
//! a line-numbered warning for one release and refuses the file from the next
//! (the flip is `V1_TOKEN_CHECKS_REFUSE`); a `key=value` entry standing where
//! either goes is refused on both sides of the flip. A `v0` file loads as it
//! always did, warned that its header names the old language and told, for
//! each token `v1` changes or refuses, the replacement and the version; it is
//! never refused for them. Warnings go through [`AppDecl::warnings`], the
//! non-fatal channel.
//!
//! An app is declared by one file. Registration is idempotent by DIFF (the
//! GQ-6 pinning discipline): a record whose bytes already exist in the fold is
//! skipped, so re-loading the file appends nothing — and can never clobber a
//! later runtime ACL update, because the fold (revocation-wins) stays the only
//! authority. Bindings diff against the `dir.bindings` fold (R9(a)), per
//! `(app, glade_id)` and only for the app the file names: a changed line
//! appends its new declaration, and a line the file no longer has appends a
//! `BindingRetraction`. So a file not loaded retracts nothing, and another
//! app's declarations are never in scope; but a second file naming the same
//! app would retract the first's bindings on every start, which is why
//! [`load_all`] refuses a start whose files name one app twice. `service`,
//! `seed` and `workspace` lines keep the plain diff: deleting one retracts
//! nothing.

use std::fs;
use std::io;
use std::path::Path;

use glade_wire::cbor;
use glade_wire::generated::Op;

use crate::registry::{BindingFold, Record, RegistryApi, RegistryError};
use crate::sysdata::{BindingDecl, BindingRetraction, CapabilityGrant, ServiceDefinition, WorkspaceEntry};

mod tail;

/// Legacy wire/declaration names remain recognizable so diagnostics can be
/// precise and numeric wire values remain reserved. New binding declarations
/// are capability-gated to the exact durable op adapters implemented by both
/// clients. SWMR assembly is delegated to the canonical shape engine.
///
/// The v1 vocabulary (§4.4 bullet 1): `crdt` binds, with the profile its tail
/// must name; `atom` (a GDL-041 engine), `message` and `window` are
/// recognised and reserved ([`RESERVED_SHAPES`]); `stream` is recognised and
/// not bindable; an exchange is authored by a `service` line.
const KNOWN_SHAPES: [&str; 9] =
    ["value", "log", "message", "stream", "exchange", "window", "swmr", "crdt", "atom"];
const BINDING_SHAPES: [&str; 4] = ["value", "log", "swmr", "crdt"];
/// Recognised shapes with nothing to bind and nothing to redirect to: the
/// refusal says they are reserved rather than only listing what binds.
const RESERVED_SHAPES: [&str; 3] = ["message", "window", "atom"];
/// The binding line as a diagnostic shows it: the five positional tokens,
/// then the optional keyword tail (R11(a)).
const BINDING_TEMPLATE: &str =
    "binding <glade_id> <shape> <authority> <zone> <retention> [ttl=<duration>] [shape-profile=<profile>]";
/// The zones a `glade-app v1` file may write (R1(a); row 31b (ii)).
const ZONES: [&str; 2] = ["commons", "private"];
/// The retentions a `glade-app v1` file may write, in the file's spelling
/// (R2(a); row 18b (ii); the hyphen-only ruling). The record stores the
/// second as `from_cursor`.
const RETENTIONS: [&str; 3] = ["latest", "from-cursor", "ttl"];
/// The retention tokens a file may not write, each with what a file that
/// writes one is told (§4.4 bullet 7; the hyphen-only ruling of 2026-09-23):
/// in a `glade-app v1` file, Step 2.5's message naming the spelling to write
/// instead; in a `glade-app v0` file, that replacement and the version the
/// token changed in (R10(a)). A file's `from_cursor` still stores what
/// `from-cursor` does.
const REFUSED_RETENTIONS: [(&str, &str, &str); 2] = [
    (
        "windowed",
        "unknown retention `windowed` (removed; use `from-cursor`)",
        "retention `windowed` was removed in `glade-app v1` (use `from-cursor`)",
    ),
    (
        "from_cursor",
        "retention `from_cursor` is the contract's spelling (use `from-cursor` in a file)",
        "retention `from_cursor` is written `from-cursor` in `glade-app v1` (`from_cursor` is the contract's spelling)",
    ),
];
/// What a `glade-app v0` header is told (R10(a)): the file loads as it always
/// did, and learns the header that names the validated grammar.
const V0_HEADER_WARNING: &str = "header `glade-app v0` names the old language; write `glade-app v1`";
/// What a file whose first declaration line is not a header is told: the
/// header to write, and the one an old file may still carry.
const HEADER_TO_WRITE: &str = "write `glade-app v1` (`glade-app v0` is accepted for old files)";
/// THE NEXT-RELEASE FLIP, for R1's row 31b (ii) and R2's row 18b (ii): "a
/// warning for one release, a hard error at the next". `false`, this release:
/// a `glade-app v1` file holding a zone or retention that `v1` does not accept
/// is told so through [`AppDecl::warnings`], ending with [`V1_REFUSED_LATER`],
/// and loads. `true`, the next release: the first such token refuses the file,
/// with the same line-numbered text less that ending. The flip is this one
/// line: nothing else changes, and the tests read this constant, so they hold
/// on both sides of it.
///
/// A release, here, is a new `version` in `node/Cargo.toml`. glade-node has
/// not had one: its version is `0.0.0`, and no tag names a node release. So
/// these warnings ship in the first release that carries this code (the first
/// version above `0.0.0`), which [`V1_WARNING_RELEASE`] names once it is cut,
/// and the release after that one sets this to `true`. A test holds the two
/// constants to `CARGO_PKG_VERSION`, so a version bump that has not decided
/// the flip fails it, and so does a flip made before that later release. A
/// `glade-app v0` file is never refused for its zone or retention, whatever
/// this says (R10(a)).
const V1_TOKEN_CHECKS_REFUSE: bool = false;
/// The node release that ships the `v1` warnings: `None` until the first
/// release (the first version above `0.0.0`) is cut, then its version. At any
/// version but `0.0.0` it must name a release. Up to and including it
/// [`V1_TOKEN_CHECKS_REFUSE`] must be `false`, and once the version is past
/// it `true` (`a_node_release_decides_the_flip`).
#[allow(dead_code)] // read by that test only
const V1_WARNING_RELEASE: Option<&str> = None;
/// How a `v1` zone or retention warning ends: what happens to the line once
/// [`V1_TOKEN_CHECKS_REFUSE`] is flipped.
const V1_REFUSED_LATER: &str = "a later node release refuses the line";
/// What a `v0` file is told after a `key=value` entry that stands where the
/// zone or the retention goes ([`misplaced`]).
const MISPLACED_V0: &str =
    "it belongs in the tail, after all five tokens (`glade-app v1` refuses it)";
/// What a binding line whose authority is `external` is told, under either
/// header: the file cannot name the source yet.
const EXTERNAL_WARNING: &str =
    "authority `external` names no source yet: the binding registers, and nothing acts on it";
/// The authority kinds (decl surface): the share is the source of record, or
/// the share caches external truth.
const AUTHORITIES: [&str; 2] = ["share", "external"];
/// The headers a node reads, each with the language it names (R10(a)). The
/// first declaration line must be one of them; anything else is refused.
const HEADERS: [(&str, AppFileVersion); 2] = [
    ("glade-app v0", AppFileVersion::V0),
    ("glade-app v1", AppFileVersion::V1),
];

/// A parsed `<app>.glade` file — pure data, inert until registered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppDecl {
    /// The language the file's header names. Parse data only: `register`
    /// never writes it, so no record carries it.
    pub version: AppFileVersion,
    pub app: String,
    pub bindings: Vec<BindingDecl>,
    /// The keyword tails of the binding lines that carry one, in file order
    /// (R11(a)). Parse data only: `register` never writes them.
    pub tails: Vec<BindingTail>,
    pub services: Vec<ServiceDefinition>,
    pub seeds: Vec<CapabilityGrant>,
    pub workspaces: Vec<WorkspaceDecl>,
    /// The non-fatal channel (R10(a)): line-numbered messages about a file
    /// that still loads. `parse` fills it in file order — a `v0` header's
    /// warning, then each binding line's zone and retention checks — and
    /// whoever loaded the file prints it (see [`AppDecl::warning_lines`]).
    pub warnings: Vec<String>,
}

impl AppDecl {
    /// The non-fatal channel as the loading boundary prints it: one line per
    /// warning, prefixed with the file's path the way `load` prefixes its
    /// errors, and marked as a warning because the node keeps booting.
    pub fn warning_lines(&self, path: impl AsRef<Path>) -> Vec<String> {
        let path = path.as_ref().display();
        self.warnings
            .iter()
            .map(|w| format!("{path}: warning: {w}"))
            .collect()
    }
}

/// The app-file language a header names (R10(a)). Both load and parse to the
/// same declarations; the binding arm's zone and retention checks branch on
/// it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppFileVersion {
    /// `glade-app v0`: the language as first shipped, and the default. It
    /// loads as it always did, with warnings: its header names the old
    /// language, and each zone or retention `v1` changes or refuses is told
    /// its replacement and the version. It is never refused for them.
    #[default]
    V0,
    /// `glade-app v1`: the validated grammar. A zone other than `commons` or
    /// `private`, or a retention other than `latest`, `from-cursor` or `ttl`,
    /// is a warning for one release and refuses the file from the next.
    V1,
}

/// A binding line's keyword tail (R11(a)), in the contract's units. Parse data
/// only, and nothing persists it in this amendment: `sysdata.BindingDecl`
/// gains no field, because taut emits every field and a new one would change
/// the bytes of every stored binding record. A key that must reach a consumer
/// takes a record kind of its own, keyed by glade id (the contract's
/// `ShapeProfileDecl`), which moves no stored byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BindingTail {
    /// The binding line's glade id.
    pub glade_id: String,
    /// `ttl=<duration>`, in milliseconds: the contract's `Retention.ttl_ms`.
    pub ttl_ms: Option<i64>,
    /// `shape-profile=<profile>`: the contract's `ShapeProfileDecl.profile`.
    pub shape_profile: Option<String>,
}

/// A declared workspace↔share association (GLP-0006 P0.S2 / audit F1): the
/// share this app serves its workspace from, plus its display name. Parse data
/// only — the REGISTERED record is the existing `WorkspaceEntry`, with the
/// registrant as the eligible host (whoever loads the file serves it).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceDecl {
    pub share: String,
    pub name: String,
}

/// Parse + validate `<app>.glade` text. Errors carry the line number — the
/// file is hand-edited, so diagnostics are part of the surface.
pub fn parse(text: &str) -> Result<AppDecl, String> {
    let mut decl = AppDecl::default();
    let mut versioned = false;
    let mut glade_ids: Vec<String> = Vec::new();

    for (idx, raw) in text.lines().enumerate() {
        let n = idx + 1;
        let line = match raw.find('#') {
            Some(i) => &raw[..i],
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();

        // The version header must be the FIRST declaration line.
        if !versioned {
            let header = toks.join(" ");
            let Some(&(_, version)) = HEADERS.iter().find(|(h, _)| *h == header) else {
                return Err(format!(
                    "line {n}: expected a header, got `{line}`: {HEADER_TO_WRITE}"
                ));
            };
            decl.version = version;
            if version == AppFileVersion::V0 {
                decl.warnings.push(format!("line {n}: {V0_HEADER_WARNING}"));
            }
            versioned = true;
            continue;
        }

        match toks[0] {
            "app" => {
                if toks.len() != 2 {
                    return Err(format!("line {n}: `app <name>`"));
                }
                if !decl.app.is_empty() {
                    return Err(format!("line {n}: duplicate `app` declaration"));
                }
                decl.app = toks[1].into();
            }
            "binding" => {
                if decl.app.is_empty() {
                    return Err(format!("line {n}: `app` must be declared before any binding"));
                }
                // A minimum: five positional tokens, then the optional tail.
                if toks.len() < 6 {
                    return Err(format!("line {n}: `{BINDING_TEMPLATE}`"));
                }
                let (glade_id, shape, retention) = (toks[1], toks[2], toks[5]);
                if !KNOWN_SHAPES.contains(&shape) {
                    return Err(format!("line {n}: unknown shape `{shape}` (known: {KNOWN_SHAPES:?})"));
                }
                if !BINDING_SHAPES.contains(&shape) {
                    return Err(unbindable(n, shape));
                }
                if !AUTHORITIES.contains(&toks[3]) {
                    return Err(format!(
                        "line {n}: unknown authority `{}` (one of {AUTHORITIES:?})",
                        toks[3]
                    ));
                }
                let tail = tail::parse(
                    n,
                    decl.version,
                    glade_id,
                    shape,
                    toks[4],
                    retention,
                    &toks[6..],
                )?;
                push_glade_id(&mut glade_ids, glade_id, n)?;
                if toks[3] == "external" {
                    decl.warnings.push(format!("line {n}: {EXTERNAL_WARNING}"));
                }
                // The zone and the retention (Step 2.6), after every refusal
                // above, so each of those keeps its message; branched by the
                // header (R10(a)).
                for told in unaccepted(toks[4], retention) {
                    match decl.version {
                        AppFileVersion::V1 => {
                            if V1_TOKEN_CHECKS_REFUSE {
                                return Err(format!("line {n}: {}", told.v1));
                            }
                            let warning = format!("line {n}: {}; {V1_REFUSED_LATER}", told.v1);
                            decl.warnings.push(warning);
                        }
                        AppFileVersion::V0 => {
                            decl.warnings.push(format!("line {n}: {}", told.v0));
                        }
                    }
                }
                decl.bindings.push(BindingDecl {
                    app: decl.app.clone(),
                    glade_id: glade_id.into(),
                    shape: shape.into(),
                    authority: toks[3].into(),
                    zone: toks[4].into(),
                    retention: stored_retention(retention).into(),
                });
                if let Some(tail) = tail {
                    decl.tails.push(tail);
                }
            }
            "service" => {
                if decl.app.is_empty() {
                    return Err(format!("line {n}: `app` must be declared before any service"));
                }
                if toks.len() != 3 {
                    return Err(format!("line {n}: `service <name> <exchange-glade-id>`"));
                }
                push_glade_id(&mut glade_ids, toks[2], n)?;
                decl.services.push(ServiceDefinition {
                    app: decl.app.clone(),
                    name: toks[1].into(),
                    glade_id: toks[2].into(),
                });
            }
            "seed" => {
                if decl.app.is_empty() {
                    return Err(format!("line {n}: `app` must be declared before any seed"));
                }
                if toks.len() != 4 {
                    return Err(format!("line {n}: `seed <principal> <share> <verb[,verb...]>`"));
                }
                decl.seeds.push(CapabilityGrant {
                    principal: toks[1].into(),
                    share: toks[2].into(),
                    verbs: toks[3].split(',').map(str::to_string).collect(),
                });
            }
            "workspace" => {
                if decl.app.is_empty() {
                    return Err(format!("line {n}: `app` must be declared before any workspace"));
                }
                if toks.len() != 3 {
                    return Err(format!("line {n}: `workspace <share> <name>`"));
                }
                if decl.workspaces.iter().any(|w| w.share == toks[1]) {
                    return Err(format!("line {n}: duplicate workspace share `{}`", toks[1]));
                }
                decl.workspaces.push(WorkspaceDecl { share: toks[1].into(), name: toks[2].into() });
            }
            other => return Err(format!("line {n}: unknown declaration `{other}`")),
        }
    }

    if !versioned {
        return Err(format!("empty file: expected a header: {HEADER_TO_WRITE}"));
    }
    if decl.app.is_empty() {
        return Err("missing `app <name>` declaration".into());
    }
    Ok(decl)
}

/// The refusal for a recognised shape that cannot be bound, with its reason
/// (§4.4 bullet 1): an exchange is authored by `service`; a reserved shape is
/// said to be reserved; any other names only what binds.
fn unbindable(n: usize, shape: &str) -> String {
    let implemented = format!("implemented: {BINDING_SHAPES:?}");
    let why = if shape == "exchange" {
        format!("{implemented}; exchange uses `service`")
    } else if RESERVED_SHAPES.contains(&shape) {
        format!("recognised and reserved, not bindable; {implemented}")
    } else {
        implemented
    };
    format!("line {n}: unsupported binding shape `{shape}` ({why})")
}

/// The retention as the record stores it (R9(b2)): a file writes
/// `from-cursor` and the record holds the contract's `from_cursor`, so the
/// store and the contract speak one vocabulary. A file's `from_cursor` stores
/// the same value; every other token is stored as written.
fn stored_retention(token: &str) -> &str {
    match token {
        "from-cursor" => "from_cursor",
        other => other,
    }
}

/// The message a `glade-app v1` file is told for `token`, if it is one of the
/// retention tokens a file may not write ([`REFUSED_RETENTIONS`]).
pub fn refused_retention(token: &str) -> Option<&'static str> {
    REFUSED_RETENTIONS.iter().find(|(t, _, _)| *t == token).map(|(_, v1, _)| *v1)
}

/// A zone or retention that `glade-app v1` does not accept, as a file in each
/// language is told it.
struct Unaccepted {
    /// Told a `glade-app v1` file: a warning for one release, then the
    /// refusal ([`V1_TOKEN_CHECKS_REFUSE`]).
    v1: String,
    /// Told a `glade-app v0` file, always as a warning: the replacement and
    /// the version the token changed in.
    v0: String,
}

/// The tokens of a binding line's `zone` and `retention` that `glade-app v1`
/// does not accept, zone first; none for a line it accepts, so the file's
/// `from-cursor` is never reported. A zone outside [`ZONES`] and a retention
/// outside [`RETENTIONS`] are named with the legal values; `windowed` and a
/// file's `from_cursor` take [`REFUSED_RETENTIONS`]' messages; a `key=value`
/// entry in either slot takes [`misplaced`]'s.
fn unaccepted(zone: &str, retention: &str) -> Vec<Unaccepted> {
    let mut out = Vec::new();
    if zone.contains('=') {
        out.push(misplaced("zone", zone));
    } else if !ZONES.contains(&zone) {
        out.push(Unaccepted {
            v1: format!("unknown zone `{zone}` (one of {ZONES:?})"),
            v0: format!("zone `{zone}` is not in `glade-app v1` (one of {ZONES:?})"),
        });
    }
    if retention.contains('=') {
        out.push(misplaced("retention", retention));
    } else if let Some(&(_, v1, v0)) = REFUSED_RETENTIONS.iter().find(|(t, _, _)| *t == retention) {
        out.push(Unaccepted { v1: v1.into(), v0: v0.into() });
    } else if !RETENTIONS.contains(&retention) {
        out.push(Unaccepted {
            v1: format!("unknown retention `{retention}` (one of {RETENTIONS:?})"),
            v0: format!("retention `{retention}` is not in `glade-app v1` (one of {RETENTIONS:?})"),
        });
    }
    out
}

/// A `key=value` entry standing where `slot` goes, which means a token is
/// missing. A `glade-app v1` file is refused for it by the tail's own check,
/// before this one and on both sides of the flip, so only a `v0` file meets
/// this: the token is stored as written, as it always was, and the file is
/// told where the entry belongs (R10(a)).
fn misplaced(slot: &str, tok: &str) -> Unaccepted {
    let v1 = tail::misplaced(slot, tok);
    let v0 = format!("`{tok}` is a key=value entry where <{slot}> goes; {MISPLACED_V0}");
    Unaccepted { v1, v0 }
}

/// A glade id is frozen once shared (GQ-6) — a duplicate within one file is a
/// declaration bug, refused at parse.
fn push_glade_id(seen: &mut Vec<String>, id: &str, line: usize) -> Result<(), String> {
    if seen.iter().any(|s| s == id) {
        return Err(format!("line {line}: duplicate glade id `{id}`"));
    }
    seen.push(id.into());
    Ok(())
}

/// Parse an `<app>.glade` file from disk. Every error names the file, as
/// `<path>: <message>`: a file that cannot be read keeps its read error's
/// kind (SUR-P3-10), and a file that breaks a rule is `InvalidData`.
pub fn load(path: impl AsRef<Path>) -> io::Result<AppDecl> {
    let path = path.as_ref();
    let text = fs::read_to_string(path)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    parse(&text).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })
}

/// Load every app file of one start, in order, before any is registered
/// (L1-14's preflight): each with [`load`], then refused when two name one
/// app. An app is declared by one file, because [`register`] takes a file as
/// its app's whole binding set: a second file naming the app would retract
/// the first's bindings, and the first the second's, on every start. The
/// refusal is one line, prefixed with the later file's path as `load`
/// prefixes its errors, naming the app and the earlier file's path.
pub fn load_all<P: AsRef<Path>>(paths: &[P]) -> io::Result<Vec<AppDecl>> {
    let decls: Vec<AppDecl> = paths.iter().map(load).collect::<io::Result<_>>()?;
    for (later, decl) in decls.iter().enumerate() {
        if let Some(earlier) = decls[..later].iter().position(|d| d.app == decl.app) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}: app `{}` is already declared by {} (an app is declared by one file)",
                    paths[later].as_ref().display(),
                    decl.app,
                    paths[earlier].as_ref().display()
                ),
            ));
        }
    }
    Ok(decls)
}

/// What one registration did — load evidence, and the idempotence observable.
#[derive(Debug, Default, PartialEq)]
pub struct Registered {
    /// records newly appended under the registrant's chain, binding
    /// retractions included.
    pub appended: usize,
    /// records whose bytes already existed in the fold (diffed away).
    pub unchanged: usize,
}

/// REGISTER a parsed declaration: every binding/service becomes an ordinary
/// record append, every ACL seed compiles to a CapabilityGrant record — all
/// attributed to `origin` (the registrant's chain). Re-registration DIFFS
/// against the existing fold: an identical record is never re-appended, so
/// loading twice is a no-op and a later runtime revocation stays authoritative.
///
/// Bindings diff against the `dir.bindings` fold (R9(a)), per
/// `(app, glade_id)`, scoped to `decl.app`: a line whose declaration is live
/// with the same bytes is unchanged; any other line appends its declaration
/// (new, changed, or declared again after a retraction); and each glade id
/// this app has live that the file no longer declares appends a
/// [`BindingRetraction`]. Retractions count in `appended`. So `decl` is its
/// app's whole binding set, and an app is declared by one file: a caller
/// registering several files loads them with [`load_all`], which refuses two
/// naming one app. A file naming an app with no binding lines retracts every
/// binding that app has live.
pub fn register(
    decl: &AppDecl,
    reg: &mut dyn RegistryApi,
    origin: &str,
) -> Result<Registered, RegistryError> {
    let ops: Vec<Op> = reg.snapshot().records.iter().map(|bytes| Op::from_cbor(&cbor::decode(bytes))).collect();
    let mut out = Registered::default();

    // Bindings: this app's live declarations, by glade id — the diff basis.
    let live = BindingFold::over(&ops).declared_by(&decl.app);
    for b in &decl.bindings {
        let rec = Record::Binding(b.clone());
        if live.get(&b.glade_id) == Some(&rec.encode()) {
            out.unchanged += 1;
        } else {
            reg.append(rec, origin)?;
            out.appended += 1;
        }
    }
    for glade_id in live.keys() {
        if !decl.bindings.iter().any(|b| &b.glade_id == glade_id) {
            let retraction = BindingRetraction { app: decl.app.clone(), glade_id: glade_id.clone() };
            reg.append(Record::Retract(retraction), origin)?;
            out.appended += 1;
        }
    }

    // Everything else, as before R9: the existing record set, as
    // (glade_id, payload bytes), is the diff basis.
    let existing: Vec<(String, Vec<u8>)> = ops.into_iter().map(|op| (op.glade_id, op.payload)).collect();
    let records = decl
        .services
        .iter()
        .map(|s| Record::Service(s.clone()))
        .chain(decl.seeds.iter().map(|g| Record::Grant(g.clone())))
        // a declared workspace registers as an ordinary WorkspaceEntry with
        // the REGISTRANT as the eligible host — the node loading the file is
        // the node that serves it (audit F1: production minting).
        .chain(decl.workspaces.iter().map(|w| {
            Record::Workspace(WorkspaceEntry {
                workspace: w.share.clone(),
                name: w.name.clone(),
                eligible_hosts: vec![origin.to_string()],
            })
        }));

    for rec in records {
        let (glade_id, payload) = (rec.glade_id().to_string(), rec.encode());
        if existing.iter().any(|(g, p)| *g == glade_id && *p == payload) {
            out.unchanged += 1;
        } else {
            reg.append(rec, origin)?;
            out.appended += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use glade_wire::generated::Shape;

    use crate::registry::{
        BindingFold, Record, Registry, RegistryApi, G_BINDINGS, G_BINDING_RETRACTIONS, G_GRANTS, G_SERVICES, HOME,
    };
    use crate::sysdata::CapabilityRevocation;

    fn grazel_file() -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/grazel-app.glade");
        std::fs::read_to_string(path).unwrap()
    }

    /// The shipped file with only its header line replaced by `header` — the
    /// edit plan Step 2.7 makes, whichever header the file carries now.
    fn grazel_file_headed(header: &str) -> String {
        let text = grazel_file();
        let mut lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.starts_with("glade-app "))
            .expect("a header line");
        lines[at] = header;
        lines.join("\n") + "\n"
    }

    /// The checked-in grazel-app.glade parses to the app-register shape grazel
    /// composes at P1.S3: 7 bindings — the 4 workspace surfaces (ws.tree/ws.files/
    /// ws.diff/term.log) PLUS the 3 pre-declared supplier surfaces (gwz.output
    /// from glade-gwz; chat.msgs + chat.groups from glade-chat, declared here so
    /// they exist node-side regardless of a running TS chat host) — 1 service, 2
    /// ACL seeds, and the declared workspace share (audit F1: the workspace↔share
    /// association is DATA).
    #[test]
    fn grazel_file_matches_the_trace_shape() {
        let decl = parse(&grazel_file()).unwrap();
        assert_eq!(decl.app, "grazel");
        assert_eq!(decl.bindings.len(), 7, "4 workspace + 3 composed-supplier surfaces");
        assert_eq!(decl.services.len(), 1, "1 service (grazel)");
        assert_eq!(decl.seeds.len(), 2, "2 ACL seeds");
        assert_eq!(decl.workspaces, vec![WorkspaceDecl { share: "ws-razel".into(), name: "razel".into() }]);
        // the directed surface the service answers (discovery.ts phase D):
        assert_eq!(decl.services[0].glade_id, "gwz.ops");
        // key surface names ride as data — the workspace tree + the composed
        // gwz.output long-op stream + the chat group-keyed log:
        assert!(decl.bindings.iter().any(|b| b.glade_id == "ws.tree" && b.shape == "value"));
        assert!(decl.bindings.iter().any(|b| b.glade_id == "ws.files" && b.shape == "swmr"));
        assert!(decl.bindings.iter().any(|b| b.glade_id == "gwz.output" && b.shape == "log"));
        assert!(decl.bindings.iter().any(|b| b.glade_id == "chat.msgs" && b.shape == "log"));
        assert!(decl.bindings.iter().any(|b| b.glade_id == "chat.groups" && b.shape == "value"));
        assert!(decl.bindings.iter().all(|b| b.app == "grazel"));
    }

    #[test]
    fn parse_diagnostics_carry_line_numbers() {
        // no header
        assert!(parse("app x\n").unwrap_err().contains("glade-app v0"));
        // unknown directive
        let e = parse("glade-app v0\napp x\nfrobnicate y\n").unwrap_err();
        assert!(e.contains("line 3") && e.contains("frobnicate"), "{e}");
        // bad shape
        let e = parse("glade-app v0\napp x\nbinding g blob share commons latest\n").unwrap_err();
        assert!(e.contains("line 3") && e.contains("blob"), "{e}");
        // Known legacy/future enum values remain decodable on the wire but are
        // not authorable bindings until an exact runtime adapter exists.
        for shape in ["message", "stream", "exchange", "window"] {
            let text = format!(
                "glade-app v0\napp x\nbinding g {shape} share commons latest\n"
            );
            let e = parse(&text).unwrap_err();
            assert!(e.contains("line 3") && e.contains("unsupported binding shape") && e.contains(shape), "{e}");
        }
        // duplicate glade id (frozen-once-shared, GQ-6)
        let e = parse(
            "glade-app v0\napp x\nbinding g value share commons latest\nbinding g log share commons latest\n",
        )
        .unwrap_err();
        assert!(e.contains("line 4") && e.contains("duplicate glade id"), "{e}");
        // declarations before `app`
        let e = parse("glade-app v0\nbinding g value share commons latest\n").unwrap_err();
        assert!(e.contains("line 2") && e.contains("`app` must be declared"), "{e}");
        // missing app entirely
        assert!(parse("glade-app v0\n").unwrap_err().contains("missing `app"));
        // workspace: arity + duplicate share
        let e = parse("glade-app v0\napp x\nworkspace ws-a\n").unwrap_err();
        assert!(e.contains("line 3") && e.contains("workspace <share> <name>"), "{e}");
        let e = parse("glade-app v0\napp x\nworkspace ws-a a\nworkspace ws-a b\n").unwrap_err();
        assert!(e.contains("line 4") && e.contains("duplicate workspace share"), "{e}");
    }

    #[test]
    fn swmr_is_an_exact_authorable_binding_shape() {
        let decl = parse(
            "glade-app v0\napp demo\nbinding ws.files swmr share commons from_cursor\n",
        )
        .unwrap();
        assert_eq!(decl.bindings[0].shape, "swmr");
    }

    /// A declared workspace registers as an ordinary WorkspaceEntry whose
    /// eligible host is the REGISTRANT — per-node data, diff-idempotent like
    /// every other registered record.
    #[test]
    fn workspace_declaration_registers_the_registrant_as_host() {
        let decl = parse("glade-app v0\napp demo\nworkspace ws-d demo-ws\n").unwrap();
        let mut reg = Registry::new();
        let out = register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(out, Registered { appended: 1, unchanged: 0 });
        assert_eq!(reg.replicas_of("ws-d"), vec!["node-1"]);
        // re-registration diffs away (same registrant, same bytes).
        let again = register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(again, Registered { appended: 0, unchanged: 1 });
    }

    /// Registration = ordinary record appends: home-share wire Ops, origin-
    /// attributed to the registrant, on the dir.bindings / dir.services /
    /// dir.grants streams — and the seeds are readable back through the SAME
    /// grants_for query any runtime grant answers (nothing about them special).
    #[test]
    fn registration_appends_ordinary_attributed_records() {
        // a non-grazel app: the loader is app-agnostic by construction.
        let decl = parse(
            "glade-app v0\napp demo\nbinding d.x value share commons latest\nservice demo d.ops\nseed alice demo read\n",
        )
        .unwrap();
        let mut reg = Registry::new();
        let out = register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(out, Registered { appended: 3, unchanged: 0 });

        let snap = reg.snapshot();
        let ops: Vec<Op> = snap.records.iter().map(|b| Op::from_cbor(&cbor::decode(b))).collect();
        assert_eq!(ops.len(), 3);
        for op in &ops {
            assert_eq!(op.share, HOME);
            assert_eq!(op.origin, "node-1", "registrant chain attribution");
        }
        let ids: Vec<&str> = ops.iter().map(|o| o.glade_id.as_str()).collect();
        assert!(ids.contains(&G_BINDINGS) && ids.contains(&G_SERVICES) && ids.contains(&G_GRANTS));
        // the compiled seed answers through the ordinary policy query:
        assert_eq!(reg.grants_for("alice", "demo"), vec!["read"]);
    }

    /// Loading twice is idempotent: registration diffs against the fold, so
    /// the second load appends nothing and the snapshot is byte-identical.
    #[test]
    fn registering_twice_appends_nothing() {
        let decl = parse(&grazel_file()).unwrap();
        let mut reg = Registry::new();
        let first = register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(first, Registered { appended: 11, unchanged: 0 }); // 7 bindings +1 service +2 seeds +1 workspace
        let snap1 = reg.snapshot();
        let second = register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(second, Registered { appended: 0, unchanged: 11 });
        assert_eq!(reg.snapshot(), snap1, "re-registration is a byte-identical no-op");
    }

    /// The fold is the only authority: a runtime revocation lands AFTER the
    /// seed, and re-registering the file cannot resurrect the grant.
    #[test]
    fn reregistration_cannot_clobber_a_runtime_revocation() {
        let decl = parse(&grazel_file()).unwrap();
        let mut reg = Registry::new();
        register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(reg.grants_for("owner", "grazel"), vec!["gwz.*", "read.*"]);
        // runtime ACL update: the admin revokes (an ordinary append).
        reg.append(
            Record::Revoke(CapabilityRevocation { principal: "owner".into(), share: "grazel".into() }),
            "node-1",
        )
        .unwrap();
        assert_eq!(reg.grants_for("owner", "grazel"), Vec::<String>::new());
        // the file seeds once; the fold rules forever.
        let again = register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(again.appended, 0, "identical seeds diff away on re-load");
        assert_eq!(reg.grants_for("owner", "grazel"), Vec::<String>::new(), "revocation stays");
    }

    /// §4.7 row 17 (R10(a)): the regression that pins plan Step 2.3 ahead of
    /// Step 2.7. Before this step (glade 559cb2c, `appdecl.rs:89-95`) `parse`
    /// compared the header for exact equality with `glade-app v0` and refused
    /// this very text, the shipped file with only its header moved to v1, with:
    ///
    /// ```text
    /// line 16: expected `glade-app v0` header, got `glade-app v1`
    /// ```
    ///
    /// `load` propagates that out of the node's `main`, so a pre-2.3 node exits
    /// at boot on any file whose header has moved. That is why this step must
    /// land, and its node be deployed, before any file's header moves
    /// (Step 2.7). The header names the language, not the content: a `v1` file
    /// declares what its `v0` twin declares — the same app, bindings,
    /// services, seeds and workspaces.
    #[test]
    fn a_v1_header_loads_as_its_v0_twin() {
        let v0 = parse(&grazel_file_headed("glade-app v0")).unwrap();
        let v1 = parse(&grazel_file_headed("glade-app v1")).unwrap();
        assert_eq!(v0.version, AppFileVersion::V0);
        assert_eq!(v1.version, AppFileVersion::V1);
        assert_eq!(v1.app, v0.app);
        assert_eq!(v1.bindings, v0.bindings);
        assert_eq!(v1.services, v0.services);
        assert_eq!(v1.seeds, v0.seeds);
        assert_eq!(v1.workspaces, v0.workspaces);
    }

    /// R10(a): both headers load, tokenized as before, and the parse records
    /// which language the file names, for validation to branch on (Step 2.6).
    #[test]
    fn both_headers_load_and_record_their_version() {
        let v0 = parse("glade-app v0\napp x\n").unwrap();
        assert_eq!(v0.version, AppFileVersion::V0);
        let v1 = parse("glade-app v1\napp x\n").unwrap();
        assert_eq!(v1.version, AppFileVersion::V1);
        let v1 = parse("# c\n\n  glade-app   v1  # the header\napp x\n").unwrap();
        assert_eq!(v1.version, AppFileVersion::V1);
    }

    /// Any other header is still refused, with its line number, the text it
    /// found, and both headers a node reads: `glade-app v1` as the one to
    /// write, `glade-app v0` as accepted for old files (SUR-P3-2).
    #[test]
    fn any_other_header_is_refused_with_its_line_naming_both() {
        let to_write = "write `glade-app v1` (`glade-app v0` is accepted for old files)";
        let cases = [
            ("glade-app v2\napp x\n", 1, "glade-app v2"),
            ("glade-app\napp x\n", 1, "glade-app"),
            ("app x\nglade-app v1\n", 1, "app x"),
            ("glade-app v1 extra\napp x\n", 1, "glade-app v1 extra"),
            ("# c\n\nglade-app V1\napp x\n", 3, "glade-app V1"),
        ];
        for (text, line, found) in cases {
            assert_eq!(
                parse(text).unwrap_err(),
                format!("line {line}: expected a header, got `{found}`: {to_write}")
            );
        }
        let empty = format!("empty file: expected a header: {to_write}");
        for text in ["", "# only a comment\n\n"] {
            assert_eq!(parse(text).unwrap_err(), empty);
        }
    }

    /// The header is parse data, never part of a record: registering the
    /// shipped file headed `v1` after its `v0` twin appends nothing, so moving
    /// the headers (Step 2.7) moves no stored byte.
    #[test]
    fn moving_the_header_to_v1_appends_no_record() {
        let mut reg = Registry::new();
        let v0 = parse(&grazel_file_headed("glade-app v0")).unwrap();
        let first = register(&v0, &mut reg, "node-1").unwrap();
        assert!(first.appended > 0);
        let snap = reg.snapshot();
        let v1 = parse(&grazel_file_headed("glade-app v1")).unwrap();
        let moved = register(&v1, &mut reg, "node-1").unwrap();
        assert_eq!((moved.appended, moved.unchanged), (0, first.appended));
        assert_eq!(reg.snapshot(), snap, "no record moved");
    }

    /// The shipped file, headed `glade-app v1` since Step 2.7, loads with no
    /// warning, because every zone and retention in it is one `v1` accepts.
    /// Headed `glade-app v0`, as it was until Step 2.7, it loads with one
    /// warning: its header's, on line 16, naming the header to write. (Step
    /// 2.3 asserted no warning under either header; Step 2.6 added the `v0`
    /// header's, deliberately; Step 2.7 moved the file to `v1`.)
    #[test]
    fn the_shipped_file_loads_with_no_warning() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/grazel-app.glade");
        let decl = load(path).unwrap();
        assert_eq!(decl.version, AppFileVersion::V1);
        assert_eq!(decl.warnings, Vec::<String>::new());
        let v0 = parse(&grazel_file_headed("glade-app v0")).unwrap();
        assert_eq!(
            v0.warnings,
            ["line 16: header `glade-app v0` names the old language; write `glade-app v1`"]
        );
    }

    /// The boundary prints each warning prefixed with the file's path, as
    /// `load` prefixes its errors. The warning is pushed by hand, so this
    /// does not depend on what the checks say.
    #[test]
    fn warnings_print_prefixed_with_the_file_path() {
        let mut decl = parse("glade-app v1\napp x\n").unwrap();
        assert_eq!(decl.warning_lines("apps/x.glade"), Vec::<String>::new());
        decl.warnings.push("line 1: a note".into());
        let printed = decl.warning_lines("apps/x.glade");
        assert_eq!(printed, vec!["apps/x.glade: warning: line 1: a note"]);
    }

    /// One binding line in a minimal file, where it is line 3.
    fn binding_line(line: &str) -> Result<AppDecl, String> {
        parse(&format!("glade-app v0\napp x\n{line}\n"))
    }

    /// §4.4 bullet 1 (row 8): `crdt` is a binding shape. It binds with its
    /// profile, which the tail makes mandatory for it (bullet 4).
    #[test]
    fn crdt_is_a_binding_shape() {
        let decl = binding_line("binding doc.body crdt share commons from-cursor shape-profile=text_crdt").unwrap();
        assert_eq!(decl.bindings[0].shape, "crdt");
    }

    /// §4.4 bullet 1 (rows 8, 9; SUR-P3-4): the recognised-but-unbindable
    /// shapes are refused with a reason each. `atom`, `message` and `window`
    /// are recognised and reserved, and say so; `exchange` keeps its redirect
    /// to the `service` line that authors it; `stream` names what binds.
    #[test]
    fn recognised_shapes_that_cannot_bind_say_why() {
        let implemented = r#"["value", "log", "swmr", "crdt"]"#;
        for shape in ["atom", "message", "window"] {
            assert_eq!(
                binding_line(&format!("binding g {shape} share commons latest")).unwrap_err(),
                format!(
                    "line 3: unsupported binding shape `{shape}` (recognised and reserved, not bindable; implemented: {implemented})"
                )
            );
        }
        assert_eq!(
            binding_line("binding g exchange share commons latest").unwrap_err(),
            format!("line 3: unsupported binding shape `exchange` (implemented: {implemented}; exchange uses `service`)")
        );
        assert_eq!(
            binding_line("binding g stream share commons latest").unwrap_err(),
            format!("line 3: unsupported binding shape `stream` (implemented: {implemented})")
        );
        let e = binding_line("binding g blob share commons latest").unwrap_err();
        assert!(e.starts_with("line 3: unknown shape `blob`") && e.contains("\"crdt\", \"atom\""), "{e}");
    }

    /// R11(a), §4.4 bullet 4: the arity check is a MINIMUM of five tokens
    /// after `binding`, and its template shows the tail. A legal five-token
    /// line never trips it, which is why the tail must also be in every
    /// grammar an author reads (bullet 12).
    #[test]
    fn the_arity_check_is_a_minimum_whose_template_shows_the_tail() {
        assert_eq!(
            binding_line("binding g value share commons").unwrap_err(),
            "line 3: `binding <glade_id> <shape> <authority> <zone> <retention> [ttl=<duration>] [shape-profile=<profile>]`"
        );
        binding_line("binding g value share commons latest").unwrap();
        binding_line("binding g value share commons ttl ttl=10m").unwrap();
    }

    /// §4.4 bullet 7 under the hyphen-only ruling: the recognised-but-refused
    /// retention tokens, each with a message naming the spelling a file
    /// writes. Defined in Step 2.5 and switched on by Step 2.6: a `v1` file
    /// is told exactly these messages, on the token's line.
    #[test]
    fn refused_retentions_are_defined_and_switched_on() {
        assert_eq!(
            refused_retention("windowed"),
            Some("unknown retention `windowed` (removed; use `from-cursor`)")
        );
        assert_eq!(
            refused_retention("from_cursor"),
            Some("retention `from_cursor` is the contract's spelling (use `from-cursor` in a file)")
        );
        for token in ["latest", "from-cursor", "ttl", "frobnicate"] {
            assert_eq!(refused_retention(token), None, "{token}");
        }
        for token in ["windowed", "from_cursor"] {
            let told = v1_reports(&v1_file(&format!("binding g log share commons {token}")));
            assert_eq!(told, [format!("line 3: {}", refused_retention(token).unwrap())], "{token}");
        }
    }

    /// No tail key reaches a record: `sysdata.BindingDecl` gains no field
    /// (taut emits every field, so a new one would change every stored
    /// binding record's bytes). A line and its tail-less twin register the
    /// same bytes.
    #[test]
    fn the_tail_reaches_no_record() {
        let with = binding_line("binding g swmr share commons ttl ttl=10m shape-profile=snapshot_delta").unwrap();
        let without = binding_line("binding g swmr share commons ttl").unwrap();
        assert_eq!(with.bindings, without.bindings);
        let (mut a, mut b) = (Registry::new(), Registry::new());
        register(&with, &mut a, "node-1").unwrap();
        register(&without, &mut b, "node-1").unwrap();
        assert_eq!(a.snapshot(), b.snapshot());
    }

    /// The live bindings as (glade_id, stored retention).
    fn live(reg: &Registry) -> Vec<(String, String)> {
        reg.bindings_of().into_iter().map(|b| (b.glade_id, b.retention)).collect()
    }
    fn pair(glade_id: &str, retention: &str) -> (String, String) {
        (glade_id.into(), retention.into())
    }
    fn ops_of(reg: &Registry) -> Vec<Op> {
        reg.snapshot().records.iter().map(|b| Op::from_cbor(&cbor::decode(b))).collect()
    }

    /// R9(a): `register` diffs a file's bindings against the FOLD, per
    /// `(app, glade_id)`, for the app the file names. A changed line appends
    /// its new declaration; a deleted line appends a retraction for its
    /// surface; a restored line declares it again; an unchanged line appends
    /// nothing. (Before R9, a restored line diffed away against its old bytes
    /// and would have stayed retracted.)
    #[test]
    fn register_diffs_bindings_per_app_against_the_fold() {
        let file = |bindings: &str| parse(&format!("glade-app v0\napp x\n{bindings}")).unwrap();
        let v1 = file("binding a value share commons latest\nbinding b log share commons from-cursor\n");
        let v2 = file("binding a value share commons ttl\nbinding c log share commons from-cursor\n");
        let mut reg = Registry::new();
        assert_eq!(register(&v1, &mut reg, "node-1").unwrap(), Registered { appended: 2, unchanged: 0 });
        // a changed, b deleted, c added: a's new declaration, c's, and b's retraction.
        assert_eq!(register(&v2, &mut reg, "node-1").unwrap(), Registered { appended: 3, unchanged: 0 });
        assert_eq!(live(&reg), vec![pair("a", "ttl"), pair("c", "from_cursor")]);
        assert_eq!(BindingFold::over(&ops_of(&reg)).retracted(), vec![("x".to_string(), "b".to_string())]);
        // back to v1: a changes back, b is declared again, c is retracted.
        assert_eq!(register(&v1, &mut reg, "node-1").unwrap(), Registered { appended: 3, unchanged: 0 });
        assert_eq!(live(&reg), vec![pair("a", "latest"), pair("b", "from_cursor")]);
        assert_eq!(BindingFold::over(&ops_of(&reg)).retracted(), vec![("x".to_string(), "c".to_string())]);
        assert_eq!(register(&v1, &mut reg, "node-1").unwrap(), Registered { appended: 0, unchanged: 2 });
        let retractions = ops_of(&reg).iter().filter(|o| o.glade_id == G_BINDING_RETRACTIONS).count();
        assert_eq!(retractions, 2, "b's and c's, each once");
    }

    /// R9(a)'s scope as `register` applies it: only the app the file names is
    /// diffed, so registering one app's file retracts nothing of another's.
    #[test]
    fn another_apps_declarations_are_never_in_scope() {
        let a = parse("glade-app v0\napp a\nbinding a.one value share commons latest\nbinding a.two value share commons latest\n").unwrap();
        let b = parse("glade-app v0\napp b\nbinding b.one log share commons from-cursor\n").unwrap();
        let a_without_two = parse("glade-app v0\napp a\nbinding a.one value share commons latest\n").unwrap();
        let mut reg = Registry::new();
        assert_eq!(register(&a, &mut reg, "node-1").unwrap(), Registered { appended: 2, unchanged: 0 });
        assert_eq!(register(&b, &mut reg, "node-1").unwrap(), Registered { appended: 1, unchanged: 0 });
        assert_eq!(register(&a_without_two, &mut reg, "node-1").unwrap(), Registered { appended: 1, unchanged: 1 });
        let ids: Vec<String> = live(&reg).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["a.one", "b.one"]);
        assert_eq!(BindingFold::over(&ops_of(&reg)).retracted(), vec![("a".to_string(), "a.two".to_string())]);
    }

    /// What the PRE-amendment `register` appended for `decl` on a fresh store,
    /// built without the code under change: bindings, services, seeds, then
    /// workspaces, each an op on its kind's stream in `origin`'s chain with
    /// lamport = seq and prev = its chain predecessor's hash — the envelope
    /// `append_returning` wrote before R9 (glade 4258c62).
    fn pre_amendment_records(decl: &AppDecl, origin: &str) -> Vec<Vec<u8>> {
        let records = decl
            .bindings
            .iter()
            .map(|b| Record::Binding(b.clone()))
            .chain(decl.services.iter().map(|s| Record::Service(s.clone())))
            .chain(decl.seeds.iter().map(|g| Record::Grant(g.clone())))
            .chain(decl.workspaces.iter().map(|w| {
                Record::Workspace(WorkspaceEntry {
                    workspace: w.share.clone(),
                    name: w.name.clone(),
                    eligible_hosts: vec![origin.to_string()],
                })
            }));
        let mut tips: BTreeMap<&str, (i64, [u8; 32])> = BTreeMap::new();
        let mut out = Vec::new();
        for rec in records {
            let (seq, prev) = match tips.get(rec.glade_id()) {
                Some(&(seq, hash)) => (seq + 1, Some(hash.to_vec())),
                None => (0, None),
            };
            let op = Op {
                share: HOME.into(),
                glade_id: rec.glade_id().into(),
                key: vec![],
                origin: origin.into(),
                seq,
                prev,
                lamport: seq,
                refs: vec![],
                shape: Shape::Log,
                payload: rec.encode(),
            };
            tips.insert(rec.glade_id(), (seq, crate::chain::op_hash(&op)));
            out.push(cbor::encode(&op.to_cbor()));
        }
        out
    }

    /// Byte stability of the envelope: on a fresh store, registering the
    /// shipped file writes exactly the records the pre-amendment `register`
    /// wrote — same order, same envelopes (the binding family's lamport rule
    /// yields seq on one origin with no retraction), same payloads.
    #[test]
    fn a_first_registration_writes_the_pre_amendment_bytes() {
        let decl = parse(&grazel_file()).unwrap();
        let mut reg = Registry::new();
        register(&decl, &mut reg, "node-1").unwrap();
        assert_eq!(reg.snapshot().records, pre_amendment_records(&decl, "node-1"));
    }

    /// R9(b2): the file writes `from-cursor` and the stored record holds the
    /// contract's `from_cursor`; a file's `from_cursor` stores the same value.
    /// Every other token is stored as written — `windowed` included. Step
    /// 2.6's checks warn about this `v0` file (its header, `from_cursor` and
    /// `windowed`) and change nothing that is stored.
    #[test]
    fn the_retention_is_stored_in_the_contracts_spelling() {
        let decl = parse(
            "glade-app v0\napp x\n\
             binding a log share commons from-cursor\n\
             binding b log share commons from_cursor\n\
             binding c value share commons latest\n\
             binding d value share commons ttl\n\
             binding e log share commons windowed\n",
        )
        .unwrap();
        let stored: Vec<&str> = decl.bindings.iter().map(|b| b.retention.as_str()).collect();
        assert_eq!(stored, ["from_cursor", "from_cursor", "latest", "ttl", "windowed"]);
        assert_eq!(decl.warnings.len(), 3, "the header, `from_cursor` and `windowed`: {:?}", decl.warnings);
        // The shipped file's five `from-cursor` lines all store the contract's spelling.
        let shipped = parse(&grazel_file()).unwrap();
        let cursor = shipped.bindings.iter().filter(|b| b.retention == "from_cursor").count();
        assert_eq!(cursor, 5);
        assert!(shipped.bindings.iter().all(|b| b.retention != "from-cursor"));
    }

    // ---- Step 2.6: the zone and retention checks (§4.7 row 7) -------------

    /// The `v0` header's warning, for a header on line 1.
    const V0_HEADER_AT_1: &str = "line 1: header `glade-app v0` names the old language; write `glade-app v1`";

    /// A `v1` file holding `lines` after its `app` line; the first is line 3.
    fn v1_file(lines: &str) -> String {
        format!("glade-app v1\napp x\n{lines}\n")
    }

    /// What a `v1` file's zone and retention checks report for `text`, in
    /// this build's channel: the file's warnings while
    /// [`V1_TOKEN_CHECKS_REFUSE`] is off (this release), or the refusal they
    /// become once it is on (the next release), which names the first
    /// violation. Each warning must end with [`V1_REFUSED_LATER`], which the
    /// refusal does not print, so it is checked and dropped here. A test that
    /// asserts through this holds on both sides of the flip, so the flip
    /// stays one line.
    fn v1_reports(text: &str) -> Vec<String> {
        if V1_TOKEN_CHECKS_REFUSE {
            vec![parse(text).unwrap_err()]
        } else {
            let ending = format!("; {V1_REFUSED_LATER}");
            let warnings = parse(text).unwrap().warnings;
            for w in &warnings {
                assert!(w.ends_with(&ending), "{w}: no refusal notice");
            }
            let cut = |w: &String| w[..w.len() - ending.len()].to_string();
            warnings.iter().map(cut).collect()
        }
    }

    /// The warnings a `v0` file holding one binding `line`, as line 3, loads
    /// with. A `v0` file is never refused for a zone or a retention.
    fn v0_warnings(line: &str) -> Vec<String> {
        binding_line(line).unwrap().warnings
    }

    /// R10(a): a `v0` header loads as before and is warned once, on its own
    /// line, with the header to write instead. A `v1` header is not warned.
    #[test]
    fn a_v0_header_is_warned_naming_the_header_to_write() {
        assert_eq!(parse("glade-app v0\napp x\n").unwrap().warnings, [V0_HEADER_AT_1]);
        assert_eq!(
            parse("# c\n\n  glade-app   v0  # the header\napp x\n").unwrap().warnings,
            ["line 3: header `glade-app v0` names the old language; write `glade-app v1`"]
        );
        assert_eq!(parse("glade-app v1\napp x\n").unwrap().warnings, Vec::<String>::new());
    }

    /// §4.7 row 7, an unknown zone (R1, row 31b (ii)): told on its line with
    /// the two zones. A `v0` file is also told the version the zone is not
    /// accepted in.
    #[test]
    fn row7_an_unknown_zone_is_told_the_two_zones() {
        for zone in ["shared", "Commons", "account"] {
            let line = format!("binding g value share {zone} latest");
            assert_eq!(
                v1_reports(&v1_file(&line)),
                [format!(r#"line 3: unknown zone `{zone}` (one of ["commons", "private"])"#)]
            );
            assert_eq!(
                v0_warnings(&line),
                [
                    V0_HEADER_AT_1.to_string(),
                    format!(r#"line 3: zone `{zone}` is not in `glade-app v1` (one of ["commons", "private"])"#)
                ]
            );
        }
    }

    /// §4.7 row 7, `windowed` (R2, row 18b (ii)): told Step 2.5's message,
    /// which names `from-cursor`. A `v0` file is told the version it was
    /// removed in.
    #[test]
    fn row7_windowed_is_told_to_write_from_cursor() {
        let line = "binding term.log log share commons windowed";
        assert_eq!(
            v1_reports(&v1_file(line)),
            ["line 3: unknown retention `windowed` (removed; use `from-cursor`)"]
        );
        assert_eq!(
            v0_warnings(line),
            [V0_HEADER_AT_1, "line 3: retention `windowed` was removed in `glade-app v1` (use `from-cursor`)"]
        );
    }

    /// §4.7 row 7, `from_cursor` written in a file (the hyphen-only ruling):
    /// told Step 2.5's message, which names `from-cursor`. A `v0` file is
    /// told the version whose spelling is the hyphen.
    #[test]
    fn row7_a_files_from_cursor_is_told_to_write_the_hyphen() {
        let line = "binding g log share commons from_cursor";
        assert_eq!(
            v1_reports(&v1_file(line)),
            ["line 3: retention `from_cursor` is the contract's spelling (use `from-cursor` in a file)"]
        );
        assert_eq!(
            v0_warnings(line),
            [
                V0_HEADER_AT_1,
                "line 3: retention `from_cursor` is written `from-cursor` in `glade-app v1` (`from_cursor` is the contract's spelling)"
            ]
        );
    }

    /// Any other retention outside `v1`'s three is told the three, in the
    /// file's spelling.
    #[test]
    fn row7_an_unknown_retention_is_told_the_three() {
        for retention in ["hourly", "Latest", "cursor"] {
            let line = format!("binding g value share commons {retention}");
            assert_eq!(
                v1_reports(&v1_file(&line)),
                [format!(r#"line 3: unknown retention `{retention}` (one of ["latest", "from-cursor", "ttl"])"#)]
            );
            assert_eq!(
                v0_warnings(&line),
                [
                    V0_HEADER_AT_1.to_string(),
                    format!(
                        r#"line 3: retention `{retention}` is not in `glade-app v1` (one of ["latest", "from-cursor", "ttl"])"#
                    )
                ]
            );
        }
    }

    /// §4.7 row 7: no diagnostic for a token `v1` accepts, and above all none
    /// for `from-cursor`, the file's spelling (R9(b2)). A `v1` file using
    /// every legal zone and retention loads with no warning; its `v0` twin
    /// with the header's only.
    #[test]
    fn row7_no_diagnostic_for_a_token_v1_accepts() {
        let lines = "binding a value share commons latest\n\
                     binding b log   share private from-cursor\n\
                     binding c swmr  share commons from-cursor shape-profile=snapshot_delta\n\
                     binding d value share private ttl ttl=10m\n\
                     binding e value share commons ttl\n\
                     binding f crdt  share commons from-cursor shape-profile=text_crdt";
        assert_eq!(parse(&v1_file(lines)).unwrap().warnings, Vec::<String>::new());
        assert_eq!(parse(&format!("glade-app v0\napp x\n{lines}\n")).unwrap().warnings, [V0_HEADER_AT_1]);
    }

    /// In this release a `v1` file's violations are warnings: the file loads,
    /// every token is stored as before, and a line with a bad zone and a bad
    /// retention is told both, zone first, each with its line and each saying
    /// a later node release refuses the line (SUR-P3-2). From the next
    /// release the first violation refuses the file, with the same text less
    /// that ending.
    #[test]
    fn a_v1_file_is_warned_this_release_and_refused_from_the_next() {
        let text = v1_file(
            "binding a value share commons latest\n\
             binding b log share shared windowed\n\
             binding c log share commons from_cursor",
        );
        let told = [
            r#"line 4: unknown zone `shared` (one of ["commons", "private"])"#,
            "line 4: unknown retention `windowed` (removed; use `from-cursor`)",
            "line 5: retention `from_cursor` is the contract's spelling (use `from-cursor` in a file)",
        ];
        if V1_TOKEN_CHECKS_REFUSE {
            assert_eq!(parse(&text).unwrap_err(), told[0]);
        } else {
            let decl = parse(&text).unwrap();
            let warned = told.map(|t| format!("{t}; a later node release refuses the line"));
            assert_eq!(decl.warnings, warned);
            let stored: Vec<(&str, &str)> =
                decl.bindings.iter().map(|b| (b.zone.as_str(), b.retention.as_str())).collect();
            assert_eq!(stored, [("commons", "latest"), ("shared", "windowed"), ("commons", "from_cursor")]);
        }
    }

    /// A `v0` file is never refused for its zone or retention, whichever way
    /// [`V1_TOKEN_CHECKS_REFUSE`] is set (R10(a)): it loads as it does today,
    /// every token stored as before, with the header's warning and then one
    /// per token `v1` changes or refuses, in file order.
    #[test]
    fn a_v0_file_loads_as_before_told_what_v1_changes() {
        let decl = parse(
            "glade-app v0\napp x\n\
             binding a value share commons latest\n\
             binding b log share shared windowed\n\
             binding c log share commons from_cursor\n\
             binding d value share private hourly\n",
        )
        .unwrap();
        assert_eq!(
            decl.warnings,
            [
                V0_HEADER_AT_1,
                r#"line 4: zone `shared` is not in `glade-app v1` (one of ["commons", "private"])"#,
                "line 4: retention `windowed` was removed in `glade-app v1` (use `from-cursor`)",
                "line 5: retention `from_cursor` is written `from-cursor` in `glade-app v1` (`from_cursor` is the contract's spelling)",
                r#"line 6: retention `hourly` is not in `glade-app v1` (one of ["latest", "from-cursor", "ttl"])"#,
            ]
        );
        let stored: Vec<(&str, &str)> = decl.bindings.iter().map(|b| (b.zone.as_str(), b.retention.as_str())).collect();
        assert_eq!(
            stored,
            [("commons", "latest"), ("shared", "windowed"), ("commons", "from_cursor"), ("private", "hourly")]
        );
    }

    /// Every refusal a line met before Step 2.6 still comes first, with its
    /// own message, in a `v1` file too: the tail's checks (a `key=value`
    /// entry where the zone or the retention goes; `ttl=` on a `windowed`
    /// line) and a duplicate glade id are refused as before, not reported as
    /// an unknown token — on both sides of the flip.
    #[test]
    fn earlier_refusals_come_before_the_token_checks() {
        assert_eq!(
            parse(&v1_file("binding g value share commons ttl=10m")).unwrap_err(),
            "line 3: `ttl=10m` is a key=value entry where <retention> goes (the tail follows all five tokens)"
        );
        assert_eq!(
            parse(&v1_file("binding g swmr share shape-profile=snapshot_delta from-cursor")).unwrap_err(),
            "line 3: `shape-profile=snapshot_delta` is a key=value entry where <zone> goes (the tail follows all five tokens)"
        );
        assert_eq!(
            parse(&v1_file("binding g log share commons windowed ttl=10m")).unwrap_err(),
            "line 3: `ttl=` needs the retention `ttl` (this line's is `windowed`)"
        );
        assert_eq!(
            parse(&v1_file("binding g value share commons latest\nbinding g log share shared windowed")).unwrap_err(),
            "line 4: duplicate glade id `g`"
        );
    }

    // ---- The amendment review's remediation, round 1 ----------------------

    /// Register `decl` under node-1: (appended, unchanged).
    fn counts(decl: &AppDecl, reg: &mut Registry) -> (usize, usize) {
        let out = register(decl, reg, "node-1").unwrap();
        (out.appended, out.unchanged)
    }

    /// STA-P2-2: a `key=value` entry where the zone or the retention goes, in
    /// a `glade-app v0` file. The token is stored as written, as before the
    /// tail (glade 559cb2c), and warned on its line with where the entry
    /// belongs; a `v0` file is never refused for it (R10(a)).
    #[test]
    fn a_v0_file_keeps_a_key_value_zone_or_retention_and_is_warned() {
        let decl = binding_line("binding g value share commons ttl=10m").unwrap();
        assert_eq!(decl.bindings[0].retention, "ttl=10m");
        let told = format!(
            "line 3: `ttl=10m` is a key=value entry where <retention> goes; {MISPLACED_V0}"
        );
        assert_eq!(decl.warnings, [V0_HEADER_AT_1.to_string(), told]);

        let decl = binding_line("binding g value share a=b latest").unwrap();
        assert_eq!(decl.bindings[0].zone, "a=b");
        let told = format!("line 3: `a=b` is a key=value entry where <zone> goes; {MISPLACED_V0}");
        assert_eq!(decl.warnings, [V0_HEADER_AT_1.to_string(), told]);

        assert_eq!(
            MISPLACED_V0,
            "it belongs in the tail, after all five tokens (`glade-app v1` refuses it)"
        );
    }

    /// STA-P2-2's `v1` twins, refused with Step 2.5's message naming the slot
    /// on both sides of the flip: the refusal is the tail's check, which
    /// [`V1_TOKEN_CHECKS_REFUSE`] does not reach.
    #[test]
    fn a_v1_file_is_refused_for_a_key_value_zone_or_retention() {
        assert_eq!(
            parse(&v1_file("binding g value share commons ttl=10m")).unwrap_err(),
            "line 3: `ttl=10m` is a key=value entry where <retention> goes (the tail follows all five tokens)"
        );
        assert_eq!(
            parse(&v1_file("binding g value share a=b latest")).unwrap_err(),
            "line 3: `a=b` is a key=value entry where <zone> goes (the tail follows all five tokens)"
        );
    }

    /// SUR-P3-7: `external` loads and registers, and is warned under either
    /// header, on both sides of the flip: the binding names no source yet, so
    /// nothing acts on it.
    #[test]
    fn external_is_warned_that_it_names_no_source() {
        let line = "binding feed.cache value external commons latest";
        let told = format!("line 3: {EXTERNAL_WARNING}");
        let decl = parse(&v1_file(line)).unwrap();
        assert_eq!(decl.warnings, [told.as_str()]);
        let mut reg = Registry::new();
        assert_eq!(counts(&decl, &mut reg), (1, 0));
        assert_eq!(reg.bindings_of()[0].authority, "external");
        assert_eq!(v0_warnings(line), [V0_HEADER_AT_1.to_string(), told]);
        assert_eq!(
            EXTERNAL_WARNING,
            "authority `external` names no source yet: the binding registers, and nothing acts on it"
        );
    }

    /// The live bindings, each as `app/glade_id shape`.
    fn live_apps(reg: &Registry) -> Vec<String> {
        let row = |b: BindingDecl| format!("{}/{} {}", b.app, b.glade_id, b.shape);
        reg.bindings_of().into_iter().map(row).collect()
    }
    /// The `notes` app of STA-P3-2's sequence, before its `app` line is renamed.
    const NOTES: &str = "glade-app v1\napp notes\n\
                         binding n.list value share commons latest\n\
                         binding n.old  log   share commons from-cursor\n";
    /// The same file with its `app` line renamed, `n.old` deleted, and
    /// `n.list` changed to a log.
    const NOTES2: &str = "glade-app v1\napp notes2\nbinding n.list log share commons from-cursor\n";

    /// SUR-P3-1 case 1, STA-P3-2: the fold is per `(app, glade_id)`, and per
    /// glade id the newest live declaration across apps stands. Renaming the
    /// `app` line starts another app and leaves the old name's declarations
    /// live, so a line deleted later brings back the old app's declaration of
    /// that surface: the outcome the format page states.
    #[test]
    fn renaming_the_app_line_leaves_the_old_names_declarations_live() {
        let mut reg = Registry::new();
        assert_eq!(counts(&parse(NOTES).unwrap(), &mut reg), (2, 0));
        // Renamed: `n.old` is not retracted, because `notes` is out of scope.
        assert_eq!(counts(&parse(NOTES2).unwrap(), &mut reg), (1, 0));
        assert_eq!(live_apps(&reg), ["notes2/n.list log", "notes/n.old log"]);
        // `n.list` deleted from the renamed file: `notes2`'s declaration is
        // retracted, and `notes`'s older one stands again, as a value.
        let deleted = parse("glade-app v1\napp notes2\n").unwrap();
        assert_eq!(counts(&deleted, &mut reg), (1, 0));
        assert_eq!(live_apps(&reg), ["notes/n.list value", "notes/n.old log"]);
    }

    /// STA-P3-2: an app is retired by loading, once, a file that names it
    /// and has no binding lines. It retracts every `binding` declaration of
    /// that app, and only that app's; loaded again, it appends nothing. It
    /// retracts nothing else: the app's `service` record stays, so its
    /// exchange stays declared (SUR-P3-9 = STA-P3-4), as the format page says.
    #[test]
    fn a_file_naming_an_app_with_no_binding_lines_retires_it() {
        let mut reg = Registry::new();
        let text = format!("{NOTES}service notes notes.ops\n");
        let notes_app = parse(&text).unwrap();
        register(&notes_app, &mut reg, "node-1").unwrap();
        register(&parse(NOTES2).unwrap(), &mut reg, "node-1").unwrap();
        let retire = parse("glade-app v1\napp notes\n").unwrap();
        assert_eq!(counts(&retire, &mut reg), (2, 0));
        assert_eq!(live_apps(&reg), ["notes2/n.list log"]);
        let retracted = BindingFold::over(&ops_of(&reg)).retracted();
        let notes = |id: &str| ("notes".to_string(), id.to_string());
        assert_eq!(retracted, [notes("n.list"), notes("n.old")]);
        let service = Record::Service(notes_app.services[0].clone()).encode();
        assert!(reg.contains(G_SERVICES, &service), "the service stays");
        assert_eq!(counts(&retire, &mut reg), (0, 0));
    }

    /// What a node version requires of the flip's two constants (COD-P3-2,
    /// COD-P3-7): `0.0.0`, no release yet, must not refuse; any other version
    /// must name the release that ships the `v1` warnings; a version at or
    /// below that release must not refuse, so the warnings do ship; and a
    /// version past it must refuse.
    fn flip_decided(version: &str, release: Option<&str>, refuse: bool) -> Result<(), String> {
        let early = "V1_TOKEN_CHECKS_REFUSE must be false";
        if version == "0.0.0" {
            if refuse {
                return Err(format!("{version} is no release: {early}"));
            }
            return Ok(());
        }
        let Some(release) = release else {
            return Err(format!("{version} is a release: set V1_WARNING_RELEASE"));
        };
        let past = numbered(version) > numbered(release);
        if past && !refuse {
            let flip = "V1_TOKEN_CHECKS_REFUSE must be true";
            return Err(format!("{version} is past {release}: {flip}"));
        }
        if !past && refuse {
            return Err(format!("{version} is not past {release}: {early}"));
        }
        Ok(())
    }

    /// A `major.minor.patch` version as numbers, so versions order as releases.
    fn numbered(version: &str) -> Vec<u64> {
        let number = |n: &str| n.parse::<u64>().ok();
        let numbers: Option<Vec<u64>> = version.split('.').map(number).collect();
        numbers.unwrap_or_else(|| panic!("`{version}` is not major.minor.patch"))
    }

    /// COD-P3-2's mechanical check: a version bump that has not decided the
    /// flip fails here, and so does a flip made before the release after the
    /// one that ships the warnings (COD-P3-7). The rule is checked on
    /// versions this build is not, then held to this build's
    /// `CARGO_PKG_VERSION`.
    #[test]
    fn a_node_release_decides_the_flip() {
        assert_eq!(flip_decided("0.0.0", None, false), Ok(()));
        assert!(flip_decided("0.1.0", None, false).is_err());
        assert_eq!(flip_decided("0.1.0", Some("0.1.0"), false), Ok(()));
        assert!(flip_decided("0.1.1", Some("0.1.0"), false).is_err());
        assert!(flip_decided("0.10.0", Some("0.9.0"), false).is_err());
        // COD-P3-7: an early flip fails, at no release and at the warnings'
        // release; the release after that one refuses.
        assert!(flip_decided("0.0.0", None, true).is_err());
        assert!(flip_decided("0.1.0", Some("0.1.0"), true).is_err());
        assert_eq!(flip_decided("0.2.0", Some("0.1.0"), true), Ok(()));
        let version = env!("CARGO_PKG_VERSION");
        let this_build = flip_decided(version, V1_WARNING_RELEASE, V1_TOKEN_CHECKS_REFUSE);
        assert_eq!(this_build, Ok(()));
    }
}
