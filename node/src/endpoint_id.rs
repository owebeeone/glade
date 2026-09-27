//! `glade-node endpoint-id --name <name>` (plan Step 4.5; the owner's ruling
//! of 2026-09-25 on 4.2b's question 3): print a node's endpoint id without
//! serving, so each machine's peer entry can be written before its first
//! start. Since the door, the accepting node must name the dialer's key.
//!
//! The command takes the instance `<root>/sys/<name>`, under the root the
//! binary read at its entry point, its name checked as a boot checks it
//! (F9), and starts no node. A key there is read,
//! its mode checked as a boot checks it, with no lock taken and nothing
//! written, so it works while the node runs. With no key yet, it creates the
//! instance directory, takes the instance lock, mints `endpoint.key` at 0600
//! with the boot's own helper, and lets the lock go. It mints nothing else:
//! the first start binds the key it finds. Its one line is the id, the only
//! line of the node's that holds one. The design is
//! `glade/dev-docs/GladeNodeAssembly.md`, "Relay configuration and the first
//! crossing (plan Step 4.5)", section 6.

use std::fs;
use std::io;
use std::path::Path;

use crate::sysdir::{load_or_create_secret, named_instance, InstanceLock};
use crate::transport::{hex, EndpointKey};

/// The key's file in an instance directory.
const KEY: &str = "endpoint.key";

/// The line the command is refused with when its arguments are not these.
const USAGE: &str = "usage: glade-node endpoint-id --name <name>";

/// `glade-node endpoint-id --name <name>`: the endpoint id of the instance
/// `<root>/sys/<name>`, `root` being the instance root the binary read at its
/// entry point, and `name` one a boot takes (F9, `sysdir::named_instance`),
/// its key minted first if it has none.
pub fn command(root: &Path, args: impl IntoIterator<Item = String>) -> io::Result<String> {
    let name = parse(args)?;
    let dir = named_instance(root, &name)?;
    if dir.join(KEY).exists() {
        return id(&dir);
    }
    fs::create_dir_all(&dir)?;
    let lock = InstanceLock::acquire(dir.join("instance.lock")).map_err(running)?;
    let id = id(&dir);
    drop(lock);
    id
}

/// The id of the key in `dir`, minted there first if there is none.
fn id(dir: &Path) -> io::Result<String> {
    let seed = load_or_create_secret(dir, KEY)?;
    Ok(hex(&EndpointKey::from_seed(seed).endpoint_id))
}

/// The command's one flag, required.
fn parse(args: impl IntoIterator<Item = String>) -> io::Result<String> {
    let mut args = args.into_iter();
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--name"), Some(name), None) => Ok(name),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE)),
    }
}

/// A node holds the instance while its key is missing: say so.
fn running(e: io::Error) -> io::Error {
    if e.kind() != io::ErrorKind::AddrInUse {
        return e;
    }
    let why = format!("{e}: a node holds the instance, and it has no {KEY}: stop the node first");
    io::Error::new(e.kind(), why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    /// The command takes `--name <name>` and nothing else.
    #[test]
    fn the_command_takes_a_name_and_nothing_else() {
        for asked in [&[][..], &["--name"], &["--out", "x"], &["--name", "n", "x"]] {
            let refused = parse(args(asked)).unwrap_err();
            assert_eq!(refused.to_string(), USAGE, "{asked:?}");
        }
        assert_eq!(parse(args(&["--name", "n"])).unwrap(), "n");
    }

    /// While a node holds an instance whose key is missing, the command
    /// mints nothing and says to stop the node.
    #[test]
    fn a_held_instance_with_no_key_is_refused() {
        let root = std::env::temp_dir().join("glade-endpoint-id-held");
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("sys").join("n");
        fs::create_dir_all(&dir).unwrap();
        let held = InstanceLock::acquire(dir.join("instance.lock")).unwrap();
        let refused = command(&root, args(&["--name", "n"])).unwrap_err();
        let says = refused.to_string();
        assert!(says.ends_with("stop the node first"), "{says}");
        assert!(!dir.join(KEY).exists(), "no key minted");
        drop(held);
        fs::remove_dir_all(&root).unwrap();
    }

    /// F9: a name that climbs out of `<root>/sys`, here `../../outside`, is
    /// refused before anything is written, and so are `..` and `.`: no key is
    /// minted, and nothing appears beside the root or in its `sys`. Before
    /// the check, each minted a key where it pointed: beside the root, in
    /// the root, and in `sys` itself.
    #[test]
    fn a_name_outside_sys_is_refused_and_mints_nothing() {
        let base = std::env::temp_dir().join("glade-endpoint-id-outside");
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        fs::create_dir_all(root.join("sys")).unwrap();
        for name in ["../../outside", "..", "."] {
            let refused = command(&root, args(&["--name", name])).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::InvalidInput, "{name}");
            let said = format!(
                "--name {name:?}: an instance name must match [A-Za-z0-9._-]{{1,63}} and be neither . nor .."
            );
            assert_eq!(refused.to_string(), said);
        }
        let names = |dir: &Path| -> Vec<String> {
            let entries = fs::read_dir(dir).unwrap();
            let name = |entry: io::Result<fs::DirEntry>| entry.unwrap().file_name();
            entries
                .map(|entry| name(entry).to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(&base), ["root"], "nothing beside the root");
        assert_eq!(names(&root), ["sys"], "nothing in the root but sys");
        assert_eq!(names(&root.join("sys")), Vec::<String>::new(), "nor in sys");
        fs::remove_dir_all(&base).unwrap();
    }
}
