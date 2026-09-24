//! §4.7 rows 9 and 10 of the reconciliation document
//! (`dev-docs/glade/GladeDeclReconciliation.md` at the glade-wz root), over
//! the real app files of its census (§4.4 bullet 6): what the first boot
//! after R9 (b2) + (a) does to a store a pre-amendment node wrote, and what
//! the scoped retraction does to two app files sharing one store.
//!
//! Where the files are. One census file lives in this repository and four in
//! sibling repositories of the glade-wz workspace, which this crate reaches as
//! `CARGO_MANIFEST_DIR/../../<repository>`. In a standalone `glade` checkout
//! there are no siblings, and these tests FAIL, naming every missing path:
//! they neither skip (a vacuous pass) nor count what they happen to find (a
//! silent miscount). Outside the workspace, run the unit tests alone with
//! `cargo test -p glade-node --lib`.

use std::path::PathBuf;

use glade_node::appdecl::{parse, register, AppDecl, AppFileVersion, Registered};
use glade_node::registry::{BindingFold, Registry, RegistryApi, G_BINDING_RETRACTIONS};
use glade_node::sysdata::BindingDecl;
use glade_wire::cbor;
use glade_wire::generated::Op;

/// The registrant: every census file registers under one node's chain.
const ORIGIN: &str = "node-1";

/// grazel's own copy of its app file, the one it ships (`grazel/src/lib.rs`).
const GRAZEL: &str = "../../grazel/apps/grazel-app.glade";
/// The second app file grazel loads when its gyld leg is on.
const GYLD: &str = "../../grazel/apps/gyld-app.glade";

/// The census's five files (§4.4 bullet 6), each with what the first
/// post-amendment boot must do to the store its pre-amendment parse filled:
/// append one record per `from-cursor` line (b2 rewrites the stored token to
/// `from_cursor`) and one for the `windowed` line Step 2.4 moved, and leave
/// every other record unchanged.
const CENSUS: [(&str, Registered); 5] = [
    (GRAZEL, Registered { appended: 5, unchanged: 6 }),
    ("../apps/grazel-app.glade", Registered { appended: 5, unchanged: 6 }),
    (GYLD, Registered { appended: 2, unchanged: 10 }),
    ("../../glade-gyld/tests/fixtures/gyld-test-app.glade", Registered { appended: 2, unchanged: 7 }),
    ("../../glade-gwz/tests/fixtures/gwz-test-app.glade", Registered { appended: 1, unchanged: 3 }),
];

fn path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Read app files by their path from this crate, failing — with every
/// missing path named — when this checkout is not inside the workspace.
fn read_all<const N: usize>(rels: [&str; N]) -> [String; N] {
    let missing: Vec<String> =
        rels.iter().map(|rel| path(rel)).filter(|p| !p.is_file()).map(|p| p.display().to_string()).collect();
    assert!(
        missing.is_empty(),
        "these tests read the census's app files from four repositories of the glade-wz workspace, \
         and this checkout has none of:\n  {}\nOutside the workspace, run `cargo test -p glade-node --lib`.",
        missing.join("\n  ")
    );
    rels.map(|rel| std::fs::read_to_string(path(rel)).unwrap())
}

/// Every record a parse registers: its bindings, services, seeds and
/// workspace entries.
fn records(decl: &AppDecl) -> usize {
    decl.bindings.len() + decl.services.len() + decl.seeds.len() + decl.workspaces.len()
}

/// Undo plan Step 2.4's one token edit (grazel 05553b4, glade 7bd5f9d):
/// `term.log`'s retention back from `from-cursor` to `windowed`. Returns the
/// text and the number of lines changed; no other census line moved in 2.4.
/// It also puts back `glade-app v0`, the header the text had then, which Step
/// 2.7 moved to `v1`; that line is not counted, being no census line.
fn before_step_2_4(text: &str) -> (String, usize) {
    let mut changed = 0;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks == ["binding", "term.log", "log", "share", "commons", "from-cursor"] {
                changed += 1;
                line.replacen("from-cursor", "windowed", 1)
            } else if toks == ["glade-app", "v1"] {
                "glade-app v0".to_string()
            } else {
                line.to_string()
            }
        })
        .collect();
    (lines.join("\n") + "\n", changed)
}

/// The PRE-amendment parse of a census file's text as it was before Step
/// 2.4: every binding field stored RAW, as the parser at glade 4258c62 did
/// (`BindingDecl { app, glade_id: toks[1], shape: toks[2], authority: toks[3],
/// zone: toks[4], retention: toks[5] }`). Today's parser still stores the
/// first four raw and rewrites only the retention (R9(b2)), so the text is
/// parsed with it and each binding's retention reset to its own line's raw
/// token; all five fields are then checked against the raw tokens, so the
/// rebuild cannot drift unnoticed. The text is headed `glade-app v0`, as it
/// was before Step 2.4: today's parser never refuses a `v0` file for a
/// retention, so this parse of `windowed` holds on both sides of the next
/// release's flip, which refuses it in a `v1` file.
fn pre_amendment(text: &str) -> AppDecl {
    let mut decl = parse(text).unwrap();
    assert_eq!(decl.version, AppFileVersion::V0, "the text as it was before Step 2.4");
    let lines: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").split_whitespace().collect::<Vec<&str>>())
        .filter(|toks| toks.first() == Some(&"binding"))
        .collect();
    assert_eq!(lines.len(), decl.bindings.len());
    for (b, toks) in decl.bindings.iter_mut().zip(&lines) {
        assert_eq!(toks.len(), 6, "a pre-amendment binding line has no tail: {toks:?}");
        b.retention = toks[5].to_string();
        let stored = [b.glade_id.as_str(), b.shape.as_str(), b.authority.as_str(), b.zone.as_str(), &b.retention];
        assert_eq!(stored, toks[1..6], "every field raw");
        assert_eq!(b.app, decl.app);
    }
    decl
}

/// The next boot: the registry rebuilt from its snapshot through
/// verify-as-ingest, as `glade-node` reloads `records.json`.
fn reboot(reg: &Registry) -> Registry {
    let (reg, rejected) = Registry::from_snapshot(&reg.snapshot());
    assert_eq!(rejected, 0);
    reg
}

/// The declarations these parses make, as `bindings_of` orders them.
fn declared(decls: &[&AppDecl]) -> Vec<BindingDecl> {
    let mut all: Vec<BindingDecl> = decls.iter().flat_map(|d| d.bindings.iter().cloned()).collect();
    all.sort_by(|a, b| a.glade_id.cmp(&b.glade_id));
    all
}

fn ops_of(reg: &Registry) -> Vec<Op> {
    reg.snapshot().records.iter().map(|b| Op::from_cbor(&cbor::decode(b))).collect()
}

fn retracted(reg: &Registry) -> Vec<(String, String)> {
    BindingFold::over(&ops_of(reg)).retracted()
}

/// §4.7 row 9 under the recorded answers {R2(a), R9(b2)}.
///
/// UNITS (SAF-P3-13): the asserted numbers count binding lines of the census,
/// with ONE REGISTRY PER FILE — each file is its own store's share of the
/// tree-wide migration cost — and the 15 is their SUM over the five files.
/// No single store appends 15: grazel-app.glade is counted once in each of
/// its two byte-identical homes, and the two fixtures are test files. The
/// owner's store is the next test.
///
/// Each file's pre-amendment parse is registered first, which fills the store
/// as a pre-amendment node left it (tokens raw, `term.log` `windowed`); the
/// file as it is now is then registered on the next boot.
#[test]
fn row9_the_first_boot_appends_15_over_the_census() {
    let texts = read_all(CENSUS.map(|(rel, _)| rel));
    let mut total = 0;
    for ((rel, want), text) in CENSUS.iter().zip(&texts) {
        let (old_text, reverted) = before_step_2_4(text);
        assert_eq!(reverted, usize::from(rel.ends_with("grazel-app.glade")), "{rel}: Step 2.4's edit");
        let pre = pre_amendment(&old_text);
        let mut reg = Registry::new();
        let first = register(&pre, &mut reg, ORIGIN).unwrap();
        assert_eq!(first, Registered { appended: records(&pre), unchanged: 0 }, "{rel}");

        let mut reg = reboot(&reg);
        let post = parse(text).unwrap();
        let out = register(&post, &mut reg, ORIGIN).unwrap();
        assert_eq!(out, *want, "{rel}");
        assert_eq!(reg.bindings_of(), declared(&[&post]), "{rel}: the post-amendment declarations are live");
        assert_eq!(retracted(&reg), vec![], "{rel}: no line was deleted");
        total += out.appended;
    }
    assert_eq!(total, 15, "binding lines appended, summed over one registry per census file");
}

/// §4.7 row 9 in the owner's store: grazel's boot with its gyld leg on
/// registers grazel-app.glade then gyld-app.glade into ONE registry, so the
/// unit here is one store, and its share of the migration is 5 + 2 = 7.
#[test]
fn row9_the_owners_two_file_store_appends_7() {
    let [grazel, gyld] = read_all([GRAZEL, GYLD]);
    let (pre_grazel, pre_gyld) =
        (pre_amendment(&before_step_2_4(&grazel).0), pre_amendment(&before_step_2_4(&gyld).0));
    let mut reg = Registry::new();
    assert_eq!(register(&pre_grazel, &mut reg, ORIGIN).unwrap(), Registered { appended: 11, unchanged: 0 });
    // the `workspace ws-razel razel` entry both files declare registers once
    assert_eq!(register(&pre_gyld, &mut reg, ORIGIN).unwrap(), Registered { appended: 11, unchanged: 1 });

    let mut reg = reboot(&reg);
    let (post_grazel, post_gyld) = (parse(&grazel).unwrap(), parse(&gyld).unwrap());
    let a = register(&post_grazel, &mut reg, ORIGIN).unwrap();
    let b = register(&post_gyld, &mut reg, ORIGIN).unwrap();
    assert_eq!((a, b), (Registered { appended: 5, unchanged: 6 }, Registered { appended: 2, unchanged: 10 }));
    assert_eq!(reg.bindings_of(), declared(&[&post_grazel, &post_gyld]));
    assert_eq!(reg.bindings_of().len(), 15);
    assert_eq!(retracted(&reg), vec![]);
}

/// Register both of grazel's app files into one fresh registry, as its boot
/// with the gyld leg on does.
fn both_registered(grazel: &AppDecl, gyld: &AppDecl) -> Registry {
    let mut reg = Registry::new();
    assert_eq!(register(grazel, &mut reg, ORIGIN).unwrap(), Registered { appended: 11, unchanged: 0 });
    assert_eq!(register(gyld, &mut reg, ORIGIN).unwrap(), Registered { appended: 11, unchanged: 1 });
    reg
}

/// §4.7 row 10, first test: grazel-app.glade then gyld-app.glade into ONE
/// registry. Registering the second file is diffed only against its own app
/// (`gyld`), so grazel's seven are not retracted: all 15 are live.
#[test]
fn row10_two_app_files_in_one_registry_are_all_live() {
    let [grazel, gyld] = read_all([GRAZEL, GYLD]);
    let (grazel, gyld) = (parse(&grazel).unwrap(), parse(&gyld).unwrap());
    let reg = both_registered(&grazel, &gyld);
    assert_eq!(reg.bindings_of(), declared(&[&grazel, &gyld]));
    assert_eq!(reg.bindings_of().len(), 15);
    assert_eq!(retracted(&reg), vec![]);
    assert!(ops_of(&reg).iter().all(|o| o.glade_id != G_BINDING_RETRACTIONS), "no retraction record");
}

/// §4.7 row 10, second test: the next boot loads grazel-app.glade alone
/// (the gyld leg switched off). A file not loaded retracts nothing: gyld's 8
/// stay live and the store does not change.
#[test]
fn row10_a_file_not_loaded_retracts_nothing() {
    let [grazel, gyld] = read_all([GRAZEL, GYLD]);
    let (grazel, gyld) = (parse(&grazel).unwrap(), parse(&gyld).unwrap());
    let before = both_registered(&grazel, &gyld).snapshot();
    let (mut reg, _) = Registry::from_snapshot(&before);
    assert_eq!(register(&grazel, &mut reg, ORIGIN).unwrap(), Registered { appended: 0, unchanged: 11 });
    assert_eq!(reg.snapshot(), before, "nothing appended");
    let gyld_live: Vec<BindingDecl> = reg.bindings_of().into_iter().filter(|b| b.app == "gyld").collect();
    assert_eq!(gyld_live, declared(&[&gyld]), "gyld's 8 untouched");
    assert_eq!(gyld_live.len(), 8);
    assert_eq!(retracted(&reg), vec![]);
}

/// §4.7 row 10, third test: one binding line deleted from gyld-app.glade
/// and both files loaded again. Exactly that surface is retracted, under
/// gyld's app; everything else is unchanged.
#[test]
fn row10_a_deleted_line_retracts_exactly_its_surface() {
    let [grazel, gyld] = read_all([GRAZEL, GYLD]);
    let (grazel_decl, gyld_decl) = (parse(&grazel).unwrap(), parse(&gyld).unwrap());
    let reg = both_registered(&grazel_decl, &gyld_decl);

    let edited: Vec<&str> = gyld
        .lines()
        .filter(|line| !line.split_whitespace().take(2).eq(["binding", "gyld.file"]))
        .collect();
    assert_eq!(edited.len(), gyld.lines().count() - 1, "exactly one line deleted");
    let edited = parse(&(edited.join("\n") + "\n")).unwrap();

    let mut reg = reboot(&reg);
    assert_eq!(register(&grazel_decl, &mut reg, ORIGIN).unwrap(), Registered { appended: 0, unchanged: 11 });
    // the retraction is gyld's one append; its 7 bindings, service, 2 seeds
    // and workspace are unchanged
    assert_eq!(register(&edited, &mut reg, ORIGIN).unwrap(), Registered { appended: 1, unchanged: 11 });
    assert_eq!(retracted(&reg), vec![("gyld".to_string(), "gyld.file".to_string())]);
    assert_eq!(reg.bindings_of(), declared(&[&grazel_decl, &edited]));
    assert_eq!(reg.bindings_of().len(), 14);
    let retractions = ops_of(&reg).iter().filter(|o| o.glade_id == G_BINDING_RETRACTIONS).count();
    assert_eq!(retractions, 1);
}
