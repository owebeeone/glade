//! The instance root is passed down, never read below the entry point
//! (ProcessGlobalsPlan Step 2.1, at the glade-wz root). `glade-node` reads
//! `GLADE_HOME` and `HOME` once, as each composition root starts, and passes
//! the root down: into `sysdir::boot` for the hand-written root, and through
//! `Settings` into `NodeStart::from_settings` for the assembled one.
//!
//! This binary points its own `GLADE_HOME` and `HOME` at a decoy directory,
//! so anything that still read them would write there, never in the real
//! `~/.glade`, and be seen. A test binary of its own, with one test, because
//! the environment is process-wide: no other test shares the process.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use glade_node::assembly::Settings;
use glade_node::claims::LEASE_TTL_MS;
use glade_node::lifecycle::{Console, NodeStart};
use glade_node::sysdir::{boot, Profile};

/// A console nothing may be said on: building a start prints nothing.
struct Silent;

impl Console for Silent {
    fn out(&self, line: &str) {
        panic!("said {line}");
    }

    fn err(&self, line: &str) {
        panic!("said on stderr {line}");
    }
}

/// A fresh directory for this test.
fn scratch() -> PathBuf {
    let nanos = UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let name = format!("glade-node-instance-root-{}-{nanos}", std::process::id());
    std::env::temp_dir().join(name)
}

/// Given a root, the hand-written root's boot and the assembled root's start
/// both put the instance under it, and neither consults the environment,
/// which names the decoy. Given none, the assembled root's start is refused
/// rather than sent to the environment for one.
#[test]
fn a_boot_given_an_instance_root_uses_it_and_not_the_environments() {
    let dir = scratch();
    let (root, decoy) = (dir.join("root"), dir.join("decoy"));
    std::fs::create_dir_all(&decoy).unwrap();
    std::env::set_var("GLADE_HOME", &decoy);
    std::env::set_var("HOME", &decoy);

    let node = boot(&root, Profile::Local, Some("t"), None, None, LEASE_TTL_MS).unwrap();
    assert_eq!(node.dir, root.join("sys").join("t"));
    assert!(node.dir.join("node.key").is_file(), "booted there");
    drop(node);

    let args = ["--profile", "peer", "0"].map(String::from);
    let settings = Settings {
        instance_root: Some(root.clone()),
        ..Settings::from_args(args)
    };
    let start = NodeStart::from_settings(settings.clone(), Vec::new(), Arc::new(Silent));
    let at = start.unwrap().instance.expect("a booted start");
    assert_eq!(at.dir, root.join("sys").join("glade-peer"));

    let rootless = Settings {
        instance_root: None,
        ..settings
    };
    let Err(refused) = NodeStart::from_settings(rootless, Vec::new(), Arc::new(Silent)) else {
        panic!("a booted start without a root was not refused");
    };
    assert_eq!(refused.kind(), ErrorKind::InvalidInput, "{refused}");

    let written: Vec<_> = std::fs::read_dir(&decoy).unwrap().collect();
    assert!(written.is_empty(), "written under the decoy: {written:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
