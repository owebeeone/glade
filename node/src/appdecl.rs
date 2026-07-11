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

/// The shapes a binding may declare (GladeSubstrateV1 §3 + decl surface).
const SHAPES: [&str; 6] = ["value", "log", "message", "stream", "exchange", "window"];
/// The authority kinds (decl surface): the share is the source of record, or
/// the share caches external truth.
const AUTHORITIES: [&str; 2] = ["share", "external"];

/// A parsed `<app>.glade` file — pure data, inert until registered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppDecl {
    pub app: String,
    pub bindings: Vec<BindingDecl>,
    pub services: Vec<ServiceDefinition>,
    pub seeds: Vec<CapabilityGrant>,
    pub workspaces: Vec<WorkspaceDecl>,
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
            if toks != ["glade-app", "v0"] {
                return Err(format!("line {n}: expected `glade-app v0` header, got `{line}`"));
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
                if toks.len() != 6 {
                    return Err(format!(
                        "line {n}: `binding <glade_id> <shape> <authority> <zone> <retention>`"
                    ));
                }
                if !SHAPES.contains(&toks[2]) {
                    return Err(format!("line {n}: unknown shape `{}` (one of {SHAPES:?})", toks[2]));
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
        return Err("empty file: expected `glade-app v0` header".into());
    }
    if decl.app.is_empty() {
        return Err("missing `app <name>` declaration".into());
    }
    Ok(decl)
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
}
