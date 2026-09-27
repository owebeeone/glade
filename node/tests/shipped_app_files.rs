//! Plan Step 2.7's done-when (`dev-docs/GladeFirstSlicePlan.md` at the
//! glade-wz root), over the five app files of the reconciliation document's
//! census (`dev-docs/glade/GladeDeclReconciliation.md`, §4.4 bullet 6): every
//! shipped file is headed `glade-app v1` and loads with no warning, because
//! every zone and retention in it is one `v1` accepts, and every seed names
//! a share one of its `workspace` lines declares (plan Step 4.3's
//! precondition 4, checked by `load_all`). Headed `glade-app v0`,
//! as each was until Step 2.7, the same declarations load with one warning,
//! the header's, which names the header to write (Step 2.6's done-when).
//!
//! Where the files are, as in `binding_census.rs`: one census file lives in
//! this repository and four in sibling repositories of the glade-wz
//! workspace, which this crate reaches as `CARGO_MANIFEST_DIR/../../<repository>`.
//! In a standalone `glade` checkout there are no siblings, and this test
//! FAILS, naming every missing path: it neither skips nor checks only the
//! files it happens to find. Outside the workspace, run the unit tests alone
//! with `cargo test -p glade-node --lib`.

use std::path::PathBuf;

use glade_node::appdecl::{load_all, parse, AppFileVersion};

/// The census's five files, by path from this crate: grazel's two app files,
/// the byte-identical twin of grazel-app.glade in this repository, and the
/// two fixtures the glade-gyld and glade-gwz tests boot a node with.
const SHIPPED: [&str; 5] = [
    "../../grazel/apps/grazel-app.glade",
    "../apps/grazel-app.glade",
    "../../grazel/apps/gyld-app.glade",
    "../../glade-gyld/tests/fixtures/gyld-test-app.glade",
    "../../glade-gwz/tests/fixtures/gwz-test-app.glade",
];

fn path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The number of `text`'s header line: its first line that is not blank or
/// a comment, as `parse` finds it.
fn header_at(text: &str) -> usize {
    let at = text.lines().position(|line| !line.split('#').next().unwrap_or("").trim().is_empty());
    at.expect("a header line") + 1
}

/// `text` with its header line replaced by `header`: with `glade-app v0`, the
/// edit plan Step 2.7 made to each file, undone.
fn headed(text: &str, header: &str) -> String {
    let at = header_at(text);
    let lines: Vec<&str> = text
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i + 1 == at {
                header
            } else {
                line
            }
        })
        .collect();
    lines.join("\n") + "\n"
}

#[test]
fn every_shipped_file_loads_with_no_warning() {
    let missing: Vec<String> =
        SHIPPED.iter().map(|rel| path(rel)).filter(|p| !p.is_file()).map(|p| p.display().to_string()).collect();
    assert!(
        missing.is_empty(),
        "this test reads the census's app files from four repositories of the glade-wz workspace, \
         and this checkout has none of:\n  {}\nOutside the workspace, run `cargo test -p glade-node --lib`.",
        missing.join("\n  ")
    );
    for rel in SHIPPED {
        let file = path(rel);
        let text = std::fs::read_to_string(&file).unwrap();
        let at = header_at(&text);

        // As the node loads it, with `load_all`, which also checks each
        // seed's share (plan Step 4.3): headed `glade-app v1`, with no warning,
        let decl = load_all(&[&file]).unwrap().remove(0);
        assert_eq!(decl.version, AppFileVersion::V1, "{rel}");
        assert_eq!(decl.warnings, Vec::<String>::new(), "{rel}");
        // so the node prints none to stderr (grazel forwards it as `[node] …`).
        assert_eq!(decl.warning_lines(&file), Vec::<String>::new(), "{rel}");

        // Headed `glade-app v0`, the same declarations load with one warning:
        // the header's, on its line.
        let v0 = parse(&headed(&text, "glade-app v0")).unwrap();
        assert_eq!(v0.version, AppFileVersion::V0, "{rel}");
        let warning = format!("line {at}: header `glade-app v0` names the old language; write `glade-app v1`");
        assert_eq!(v0.warnings, [warning], "{rel}");
        assert_eq!(v0.bindings, decl.bindings, "{rel}: the header moves no declaration");
    }
}

/// The desk's start (F4, the owner's ruling of 2026-09-27): grazel passes
/// the node both of its app files, `--app apps/grazel-app.glade --app
/// apps/gyld-app.glade`, and `load_all` checks them together. So a `revoke`
/// line in either could cancel a seed in the other, which loading each alone
/// cannot show. Loaded together, neither is warned of anything.
#[test]
fn the_desks_two_files_load_together_with_no_warning() {
    let files = [path(SHIPPED[0]), path(SHIPPED[2])];
    let decls = load_all(&files).unwrap();
    for (file, decl) in files.iter().zip(&decls) {
        assert_eq!(decl.warning_lines(file), Vec::<String>::new());
    }
}
