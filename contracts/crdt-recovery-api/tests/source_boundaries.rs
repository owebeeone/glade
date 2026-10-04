//! Parse all source branches, including disabled cfg contents, without compiling them.
use std::path::Path;
use syn::{Attribute, ItemMod, visit::Visit};
struct Boundaries(Vec<String>);
impl<'ast> Visit<'ast> for Boundaries {
    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        if attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr") {
            self.0.push("unbraced conditional declaration".into());
        }
        syn::visit::visit_attribute(self, attribute);
    }
    fn visit_item_mod(&mut self, module: &'ast ItemMod) {
        if let Some((_, items)) = &module.content {
            // Only attrs owned by the enclosing inline platform module are exempt.
            // Visit every child, including disabled fields/associated/foreign items.
            for attribute in &module.attrs {
                if !attribute.path().is_ident("cfg") && !attribute.path().is_ident("cfg_attr") {
                    self.visit_attribute(attribute);
                }
            }
            for item in items {
                self.visit_item(item);
            }
        } else {
            syn::visit::visit_item_mod(self, module);
        }
    }
}
fn violations(source: &str) -> Vec<String> {
    let file = syn::parse_file(source).unwrap();
    let mut check = Boundaries(vec![]);
    check.visit_file(&file);
    check.0
}
fn files(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut result = vec![];
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else if path.extension().is_some_and(|x| x == "rs") {
            result.push(path);
        }
    }
    result
}
#[test]
fn checks_disabled_branch_instead_of_skipping_cfg() {
    assert_eq!(
        violations("#[cfg(never)] mod platform { #[cfg(windows)] use missing::Thing; }").len(),
        1
    );
    assert_eq!(
        violations("#[cfg_attr(never, cfg(windows))] use missing::Thing;").len(),
        1
    );
    assert!(violations("#[cfg(windows)] mod platform { use missing::Thing; }").is_empty());
}
#[test]
fn tranche_sources_have_explicit_conditional_boundaries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for directory in ["src", "tests"] {
        for file in files(&root.join(directory)) {
            assert!(
                violations(&std::fs::read_to_string(&file).unwrap()).is_empty(),
                "{}",
                file.display()
            );
        }
    }
}

#[test]
fn disabled_associated_foreign_and_field_declarations_are_checked() {
    for source in [
        "#[cfg(never)] mod platform { struct S { #[cfg(windows)] field:u8 } }",
        "#[cfg(never)] mod platform { enum E { #[cfg(windows)] V } }",
        "#[cfg(never)] mod platform { impl S { #[cfg(windows)] fn f() {} } }",
        "#[cfg(never)] mod platform { trait T { #[cfg(windows)] fn f(); } }",
        "#[cfg(never)] mod platform { extern \"C\" { #[cfg(windows)] fn f(); } }",
        "#[cfg(never)] mod platform;",
    ] {
        assert_eq!(violations(source).len(), 1, "{source}");
    }
}

#[test]
fn actual_node_independent_subtree_and_disabled_branches_are_checked() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node");
    for file in files(&root.join("src/independent"))
        .into_iter()
        .chain(files(&root.join("tests/ic3_disk_remediation")))
        .chain([
            root.join("tests/ic3_boundary.rs"),
            root.join("tests/ic3_disk.rs"),
        ])
    {
        assert!(
            violations(&std::fs::read_to_string(&file).unwrap()).is_empty(),
            "{}",
            file.display()
        );
    }
}
