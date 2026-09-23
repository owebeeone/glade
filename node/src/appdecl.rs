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
//! glade-app v0                 # version header, first declaration line
//! app <name>                   # exactly once, before any declaration
//! binding <glade_id> <shape> <authority> <zone> <retention>
//! service <name> <exchange-glade-id>
//! seed <principal> <share> <verb[,verb...]>
//! workspace <share> <name>     # a workspace share this app serves from
//! ```
//!
//! The header names the file's language (R10(a)): `glade-app v0` or
//! `glade-app v1`, recorded as [`AppDecl::version`]; any other first
//! declaration line is refused with its line number. Nothing validates by
//! version yet, so the two parse identically. A file that loads can still
//! carry messages through [`AppDecl::warnings`], the non-fatal channel.
//!
//! Registration is idempotent by DIFF (the GQ-6 pinning discipline): a record
//! whose bytes already exist in the fold is skipped, so re-loading the file
//! appends nothing — and can never clobber a later runtime ACL update, because
//! the fold (revocation-wins) stays the only authority.

use std::fs;
use std::io;
use std::path::Path;

use glade_wire::cbor;
use glade_wire::generated::Op;

use crate::registry::{Record, RegistryApi, RegistryError};
use crate::sysdata::{BindingDecl, CapabilityGrant, ServiceDefinition, WorkspaceEntry};

/// Legacy wire/declaration names remain recognizable so diagnostics can be
/// precise and numeric wire values remain reserved. New binding declarations
/// are capability-gated to the exact durable op adapters implemented by both
/// clients. SWMR assembly is delegated to the canonical shape engine.
const KNOWN_SHAPES: [&str; 7] = ["value", "log", "message", "stream", "exchange", "window", "swmr"];
const BINDING_SHAPES: [&str; 3] = ["value", "log", "swmr"];
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
    pub services: Vec<ServiceDefinition>,
    pub seeds: Vec<CapabilityGrant>,
    pub workspaces: Vec<WorkspaceDecl>,
    /// The non-fatal channel (R10(a)): line-numbered messages about a file
    /// that still loads. `parse` fills it; whoever loaded the file prints it
    /// (see [`AppDecl::warning_lines`]). Nothing produces one yet: the `v0`
    /// header's warning and the token checks arrive with validation.
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

/// The app-file language a header names (R10(a)). Both load; validation's
/// binding arm branches on this once `v1` is the validated grammar, and until
/// then the two parse identically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppFileVersion {
    /// `glade-app v0`: the language as first shipped, and the default.
    #[default]
    V0,
    /// `glade-app v1`: the validated grammar, parsed as `v0` until
    /// validation lands.
    V1,
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
                let expected = expected_headers();
                return Err(format!(
                    "line {n}: expected {expected} header, got `{line}`"
                ));
            };
            decl.version = version;
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
                if toks.len() != 6 {
                    return Err(format!(
                        "line {n}: `binding <glade_id> <shape> <authority> <zone> <retention>`"
                    ));
                }
                if !KNOWN_SHAPES.contains(&toks[2]) {
                    return Err(format!("line {n}: unknown shape `{}` (known: {KNOWN_SHAPES:?})", toks[2]));
                }
                if !BINDING_SHAPES.contains(&toks[2]) {
                    return Err(format!(
                        "line {n}: unsupported binding shape `{}` (implemented: {BINDING_SHAPES:?}; exchange uses `service`)",
                        toks[2]
                    ));
                }
                if !AUTHORITIES.contains(&toks[3]) {
                    return Err(format!(
                        "line {n}: unknown authority `{}` (one of {AUTHORITIES:?})",
                        toks[3]
                    ));
                }
                push_glade_id(&mut glade_ids, toks[1], n)?;
                decl.bindings.push(BindingDecl {
                    app: decl.app.clone(),
                    glade_id: toks[1].into(),
                    shape: toks[2].into(),
                    authority: toks[3].into(),
                    zone: toks[4].into(),
                    retention: toks[5].into(),
                });
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
        let expected = expected_headers();
        return Err(format!("empty file: expected {expected} header"));
    }
    if decl.app.is_empty() {
        return Err("missing `app <name>` declaration".into());
    }
    Ok(decl)
}

/// The headers a node reads, as a diagnostic names them.
fn expected_headers() -> String {
    let quoted: Vec<String> = HEADERS.iter().map(|(h, _)| format!("`{h}`")).collect();
    quoted.join(" or ")
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

/// Parse an `<app>.glade` file from disk.
pub fn load(path: impl AsRef<Path>) -> io::Result<AppDecl> {
    let text = fs::read_to_string(&path)?;
    parse(&text).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {e}", path.as_ref().display()),
        )
    })
}

/// What one registration did — load evidence, and the idempotence observable.
#[derive(Debug, Default, PartialEq)]
pub struct Registered {
    /// records newly appended under the registrant's chain.
    pub appended: usize,
    /// records whose bytes already existed in the fold (diffed away).
    pub unchanged: usize,
}

/// REGISTER a parsed declaration: every binding/service becomes an ordinary
/// record append, every ACL seed compiles to a CapabilityGrant record — all
/// attributed to `origin` (the registrant's chain). Re-registration DIFFS
/// against the existing fold: an identical record is never re-appended, so
/// loading twice is a no-op and a later runtime revocation stays authoritative.
pub fn register(
    decl: &AppDecl,
    reg: &mut dyn RegistryApi,
    origin: &str,
) -> Result<Registered, RegistryError> {
    // The existing record set, as (glade_id, payload bytes) — the diff basis.
    let existing: Vec<(String, Vec<u8>)> = reg
        .snapshot()
        .records
        .iter()
        .map(|bytes| {
            let op = Op::from_cbor(&cbor::decode(bytes));
            (op.glade_id, op.payload)
        })
        .collect();

    let records = decl
        .bindings
        .iter()
        .map(|b| Record::Binding(b.clone()))
        .chain(decl.services.iter().map(|s| Record::Service(s.clone())))
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

    let mut out = Registered::default();
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
    use crate::registry::{Record, Registry, RegistryApi, G_BINDINGS, G_GRANTS, G_SERVICES, HOME};
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
    /// found, and both headers a node reads.
    #[test]
    fn any_other_header_is_refused_with_its_line_naming_both() {
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
                format!(
                    "line {line}: expected `glade-app v0` or `glade-app v1` header, got `{found}`"
                )
            );
        }
        for text in ["", "# only a comment\n\n"] {
            assert_eq!(
                parse(text).unwrap_err(),
                "empty file: expected `glade-app v0` or `glade-app v1` header"
            );
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

    /// The non-fatal channel exists and is empty for the shipped file under
    /// either header: nothing produces a warning until validation (Step 2.6).
    #[test]
    fn the_shipped_file_loads_with_no_warning() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../apps/grazel-app.glade");
        assert_eq!(load(path).unwrap().warnings, Vec::<String>::new());
        let v1 = parse(&grazel_file_headed("glade-app v1")).unwrap();
        assert_eq!(v1.warnings, Vec::<String>::new());
    }

    /// The boundary prints each warning prefixed with the file's path, as
    /// `load` prefixes its errors. Nothing produces a warning yet, so this one
    /// is pushed by hand.
    #[test]
    fn warnings_print_prefixed_with_the_file_path() {
        let mut decl = parse("glade-app v1\napp x\n").unwrap();
        assert_eq!(decl.warning_lines("apps/x.glade"), Vec::<String>::new());
        decl.warnings.push("line 1: a note".into());
        let printed = decl.warning_lines("apps/x.glade");
        assert_eq!(printed, vec!["apps/x.glade: warning: line 1: a note"]);
    }
}
