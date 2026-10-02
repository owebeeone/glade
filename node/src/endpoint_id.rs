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
//!
//! `node-id --name <name>` shares the same validation and locking contract,
//! but reads or mints only `node.key` and prints the node signing identity used
//! in grant declarations. It does not mint an endpoint key or start a node.

use std::fs;
use std::io;
use std::path::Path;

use crate::signing;
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
    identity_command(root, args, KEY, USAGE, |seed| {
        EndpointKey::from_seed(seed).endpoint_id
    })
}

/// `glade-node node-id --name <name>`: print the grant principal before boot.
/// Only `node.key` is minted; existing keys are read without taking the lock.
/// Naming, permissions and missing-key locking match `endpoint-id`.
pub fn node_command(root: &Path, args: impl IntoIterator<Item = String>) -> io::Result<String> {
    identity_command(
        root,
        args,
        "node.key",
        "usage: glade-node node-id --name <name>",
        |seed| signing::public_key(&seed),
    )
}

fn identity_command(
    root: &Path,
    args: impl IntoIterator<Item = String>,
    key: &str,
    usage: &str,
    public: impl Fn([u8; 32]) -> [u8; 32],
) -> io::Result<String> {
    let name = parse_with_usage(args, usage)?;
    let dir = named_instance(root, &name)?;
    let id = || load_or_create_secret(&dir, key).map(|seed| hex(&public(seed)));
    if dir.join(key).exists() {
        return id();
    }
    fs::create_dir_all(&dir)?;
    let lock = InstanceLock::acquire(dir.join("instance.lock")).map_err(|error| {
        if error.kind() != io::ErrorKind::AddrInUse {
            return error;
        }
        let why =
            format!("{error}: a node holds the instance, and it has no {key}: stop the node first");
        io::Error::new(error.kind(), why)
    })?;
    let result = id();
    drop(lock);
    result
}

/// The command's one flag, required.
fn parse_with_usage(args: impl IntoIterator<Item = String>, usage: &str) -> io::Result<String> {
    let mut args = args.into_iter();
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--name"), Some(name), None) => Ok(name),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, usage)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn node_id_mints_only_the_node_key_and_matches_boot() {
        let root = std::env::temp_dir().join(format!("glade-node-id-mint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let first = node_command(&root, args(&["--name", "n"])).unwrap();
        let dir = root.join("sys/n");
        assert!(dir.join("node.key").is_file());
        assert!(!dir.join("endpoint.key").exists());
        assert!(!dir.join("records.json").exists());
        assert!(!dir.join("instance.lock").exists());
        let held = InstanceLock::acquire(dir.join("instance.lock")).unwrap();
        assert_eq!(node_command(&root, args(&["--name", "n"])).unwrap(), first);
        drop(held);
        let boot = crate::sysdir::boot_at(dir.clone(), "owner").unwrap();
        assert_eq!(boot.node_id, first);
        drop(boot);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn node_id_refuses_bad_names_arguments_and_a_missing_key_under_lock() {
        let root = std::env::temp_dir().join(format!("glade-node-id-held-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(node_command(&root, args(&["--name", "../outside"])).is_err());
        assert!(!root.exists());
        assert_eq!(
            node_command(&root, args(&[])).unwrap_err().to_string(),
            "usage: glade-node node-id --name <name>"
        );
        let dir = root.join("sys/n");
        fs::create_dir_all(&dir).unwrap();
        let held = InstanceLock::acquire(dir.join("instance.lock")).unwrap();
        let error = node_command(&root, args(&["--name", "n"])).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        assert!(error.to_string().contains("no node.key"));
        assert!(!dir.join("node.key").exists());
        drop(held);
        fs::write(dir.join("node.key"), [0; 31]).unwrap();
        assert!(node_command(&root, args(&["--name", "n"])).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// The command takes `--name <name>` and nothing else.
    #[test]
    fn the_command_takes_a_name_and_nothing_else() {
        for asked in [&[][..], &["--name"], &["--out", "x"], &["--name", "n", "x"]] {
            let refused = parse_with_usage(args(asked), USAGE).unwrap_err();
            assert_eq!(refused.to_string(), USAGE, "{asked:?}");
        }
        assert_eq!(
            parse_with_usage(args(&["--name", "n"]), USAGE).unwrap(),
            "n"
        );
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
                "--name {name:?}: an instance name must match [A-Za-z0-9._-]{{1,63}} and not end in a dot"
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
