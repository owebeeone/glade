//! The node's recovery key (plan Step 4.1c; `GladeNodeSigning.md` D10 (a),
//! the ruling `key_custody = recovery_keys`): a separate Ed25519 key whose
//! public half the node commits to in its own chain, on `dir.recovery-keys`,
//! and whose secret half is written where the operator names, and nowhere
//! else. The node keeps no copy. Nothing reads the key until rotation, plan
//! Step 5.1's gap. The design is `glade/dev-docs/GladeNodeAssembly.md`,
//! "Custody and the local overlay's check".
//!
//! A node that has booted commits one with `glade-node recovery --name <name>
//! --out <path>` on its stopped instance ([`command`]); a new node can take
//! `--recovery-out <path>` at its first boot (`sysdir::boot_at_with`). Until
//! it has committed one, each start warns ([`warning`]). Nothing here reads
//! the process's environment, working directory or path: the binary reads
//! what it needs once, at its entry point, and hands it in.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::registry::{entry_sync, Record, Registry, RegistryApi};
use crate::signing;
use crate::sysdata::NodeRecoveryKey;
use crate::sysdir::{boot_at, named_instance, Boot};
use crate::transport::hex;

/// How the line a start prints begins while its node has committed no
/// recovery key ([`warning`]).
pub const NOT_COMMITTED: &str = "no recovery key is committed for this node";

/// The line the command is refused with when its arguments are not these.
const USAGE: &str =
    "usage: glade-node recovery --name <name> --out <an absolute path outside GLADE_HOME>";

/// A recovery key committed, and where its secret half was written.
#[derive(Debug, PartialEq)]
pub struct Committed {
    pub key: String,
    pub file: PathBuf,
}

/// The line the command, and a first boot given `--recovery-out`, print once
/// the key is committed.
impl fmt::Display for Committed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (key, file) = (&self.key, self.file.display());
        write!(
            f,
            "recovery key {key} committed; wrote its secret to {file}; this node keeps no copy: move the file offline now"
        )
    }
}

/// `glade-node recovery --name <name> --out <path>`: commit a recovery key for
/// the stopped instance `<root>/sys/<name>`, `root` being the instance root
/// the binary read at its entry point, and `name` one a boot takes (F9,
/// `sysdir::named_instance`). Returns the lines to print. It boots the
/// instance as a start does, so it takes the instance lock, and writes the
/// secret before it saves the commitment.
pub fn command(root: &Path, args: impl IntoIterator<Item = String>) -> io::Result<Vec<String>> {
    let (name, out) = parse(args)?;
    let sys = root.join("sys");
    let dir = named_instance(root, &name)?;
    if !dir.join("node.key").exists() {
        let sys = sys.display();
        let why = format!("no instance {name} in {sys}: the command commits a recovery key for an instance that has booted");
        return Err(io::Error::new(io::ErrorKind::NotFound, why));
    }
    let out = check_out(root, &out)?;
    let mut boot = boot_at(dir, "local").map_err(stop_first)?;
    let node = boot.node_id.clone();
    if let Some(key) = boot.registry.recovery_key(&node) {
        let why = format!("node {node} has committed recovery key {key} already, and commits one");
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, why));
    }
    let (record, committed) = mint(&node, &out)?;
    let Boot {
        registry, store, ..
    } = &mut boot;
    let appended = registry.accept(store, |staged| append(staged, record, &node));
    appended.map_err(|e| unsaved(e, &out))?;
    Ok(vec![format!("node {node}"), committed.to_string()])
}

/// The command's two flags, each required.
fn parse(args: impl IntoIterator<Item = String>) -> io::Result<(String, PathBuf)> {
    let usage = || io::Error::new(io::ErrorKind::InvalidInput, USAGE);
    let (mut name, mut out) = (None, None);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let slot = match arg.as_str() {
            "--name" => &mut name,
            "--out" => &mut out,
            _ => return Err(usage()),
        };
        *slot = Some(args.next().ok_or_else(usage)?);
    }
    match (name, out) {
        (Some(name), Some(out)) => Ok((name, PathBuf::from(out))),
        _ => Err(usage()),
    }
}

/// The instance lock refuses the command while the node runs: say so.
fn stop_first(e: io::Error) -> io::Error {
    if e.kind() != io::ErrorKind::AddrInUse {
        return e;
    }
    io::Error::new(e.kind(), format!("{e}: stop the node first"))
}

/// A save that failed after the secret was written: the file is not known to
/// be committed, and is kept, since a save whose outcome is unknown may have
/// landed.
fn unsaved(e: io::Error, out: &Path) -> io::Error {
    let out = out.display();
    let why = format!(
        "{e}; {out} was written, and its key is not known to be committed: while a start still warns that no recovery key is committed, the file is unused"
    );
    io::Error::new(e.kind(), why)
}

/// Append `record` to `registry` as `node`'s next op on its chain.
fn append(registry: &mut Registry, record: NodeRecoveryKey, node: &str) -> io::Result<()> {
    let appended = registry.append(Record::Recovery(record), node);
    let why = |e| format!("registry append rejected: {e:?}");
    appended.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, why(e)))
}

/// The instance root of the instance at `dir`, `<root>/sys/<name>`.
pub(crate) fn root_of(dir: &Path) -> &Path {
    dir.parent().and_then(Path::parent).unwrap_or(dir)
}

/// Where a recovery key's secret may go: `out`, the links in its directory
/// resolved, if it is absolute, its directory exists, nothing is there, and
/// it is not inside `root`, the instance root (`GLADE_HOME`). Checked before
/// anything is written. A relative path is refused: resolving it would read
/// the working directory, which only a program's entry point may do.
pub fn check_out(root: &Path, out: &Path) -> io::Result<PathBuf> {
    let refused = |kind, why: &str| io::Error::new(kind, format!("{}: {why}", out.display()));
    if !out.is_absolute() {
        return Err(refused(io::ErrorKind::InvalidInput, "not an absolute path"));
    }
    let (Some(dir), Some(file)) = (out.parent(), out.file_name()) else {
        return Err(refused(io::ErrorKind::InvalidInput, "names no file"));
    };
    let unusable = |e: io::Error| refused(e.kind(), &format!("its directory cannot be used ({e})"));
    let path = fs::canonicalize(dir).map_err(unusable)?.join(file);
    if fs::symlink_metadata(&path).is_ok() {
        let why = "something is there already, and a recovery key is never written over it";
        return Err(refused(io::ErrorKind::AlreadyExists, why));
    }
    let roots = [Some(root.to_path_buf()), fs::canonicalize(root).ok()];
    if roots.iter().flatten().any(|root| path.starts_with(root)) {
        let why = format!(
            "inside GLADE_HOME ({}); name a path outside it",
            root.display()
        );
        return Err(refused(io::ErrorKind::InvalidInput, &why));
    }
    Ok(path)
}

/// The refusal of `--recovery-out` at a boot that is not the instance's first.
pub(crate) fn not_first_boot(dir: &Path, out: &Path) -> io::Error {
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    let (dir, out) = (dir.display(), out.display());
    let why = format!(
        "--recovery-out is taken at a node's first boot only, and {dir} has booted: stop it and run glade-node recovery --name {name} --out {out}"
    );
    io::Error::new(io::ErrorKind::InvalidInput, why)
}

/// Mint a recovery key for `node`: write its secret half to `out`, a new
/// file, 0600 on Unix, synced with its directory entry; and return the record
/// that commits to its public half, for the caller to append, and what to
/// print once it is saved. The secret is not kept.
pub(crate) fn mint(node: &str, out: &Path) -> io::Result<(NodeRecoveryKey, Committed)> {
    let seed = signing::random_seed()?;
    let key = hex(&signing::public_key(&seed));
    write_secret(out, &seed)?;
    let record = NodeRecoveryKey {
        node: node.into(),
        recovery_key: key.clone(),
    };
    let file = out.to_path_buf();
    Ok((record, Committed { key, file }))
}

/// Write `seed` to a new file at `path`, and sync it and its directory
/// entry. A file already there is refused; a write that fails removes what
/// it created.
fn write_secret(path: &Path, seed: &[u8; 32]) -> io::Result<()> {
    let mut file = platform::create_new(path).map_err(|e| {
        let why = format!("{}: cannot be created ({e})", path.display());
        io::Error::new(e.kind(), why)
    })?;
    if let Err(e) = file.write_all(seed).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(e);
    }
    entry_sync::sync(path.parent().unwrap_or(path))
}

/// The line a start prints after its boot lines while `boot`'s node has
/// committed no recovery key: exactly what to run for this instance, under
/// the instance root it was booted from. `program` is the running program's
/// path, which the binary read at its entry point; without one, the line
/// names `glade-node`. `None` once a key is committed.
pub fn warning(boot: &Boot, program: Option<&Path>) -> Option<String> {
    if boot.registry.recovery_key(&boot.node_id).is_some() {
        return None;
    }
    let name = boot.dir.file_name().unwrap_or_default().to_string_lossy();
    let root = root_of(&boot.dir).display().to_string();
    let program = program.map_or_else(|| "glade-node".into(), |p| p.display().to_string());
    let (root, program, name) = (shell(&root), shell(&program), shell(&name));
    Some(format!(
        "{NOT_COMMITTED}: stop it, then run GLADE_HOME={root} {program} recovery --name {name} --out <an absolute path outside GLADE_HOME>"
    ))
}

/// `word` as a POSIX shell reads it back: as it is when no character in it is
/// one a shell splits or expands, else single-quoted.
fn shell(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

// A new file's mode is a Unix notion. Each platform's branch is one braced
// module, so the condition encloses the whole section.
#[cfg(unix)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    /// A new file at `path`, mode 0600; one already there is refused.
    pub(super) fn create_new(path: &Path) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        options.open(path)
    }
}

#[cfg(not(unix))]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::path::Path;

    /// A new file at `path`, with the platform's default permissions, as
    /// `node.key` gets off Unix; one already there is refused.
    pub(super) fn create_new(path: &Path) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).open(path)
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::LEASE_TTL_MS;
    use crate::sysdir::boot_at_with;

    /// A fresh directory for one test, as `(root, offline)`: an instance root,
    /// and a directory outside it for the secret.
    fn fresh(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glade-recovery-{name}"));
        let _ = fs::remove_dir_all(&dir);
        let (root, offline) = (dir.join("root"), dir.join("offline"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&offline).unwrap();
        (root, offline)
    }

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    /// The secret a file holds, and the key it commits to.
    fn secret_of(path: &Path) -> ([u8; 32], String) {
        let secret: [u8; 32] = fs::read(path).unwrap().try_into().unwrap();
        (secret, hex(&signing::public_key(&secret)))
    }

    /// Every file under `dir`, at any depth.
    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(files_under(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    /// D10 (a)'s command on a stopped instance: it commits the key in the
    /// node's own chain, where the next boot verifies it and no longer
    /// warns, and writes the secret, 32 bytes whose public half is that key,
    /// to the file named and to no file in the instance. A second run is
    /// refused, naming the key, and writes nothing.
    #[test]
    fn the_command_commits_the_key_and_writes_its_secret_only_where_named() {
        let (root, offline) = fresh("commit");
        let dir = root.join("sys").join("n");
        let node = boot_at(dir.clone(), "gianni").unwrap().node_id.clone();
        let out = offline.join("n.recovery");
        let named = args(&["--name", "n", "--out", out.to_str().unwrap()]);
        let lines = command(&root, named).unwrap();
        let (secret, key) = secret_of(&out);
        let file = fs::canonicalize(&out).unwrap();
        let committed = Committed {
            key: key.clone(),
            file,
        };
        assert_eq!(lines, [format!("node {node}"), committed.to_string()]);
        let boot = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!(boot.registry.recovery_key(&node), Some(key.clone()));
        assert_eq!(warning(&boot, None), None, "no warning once committed");
        drop(boot);
        for file in files_under(&dir) {
            let held = fs::read(&file).unwrap();
            let copy = held.windows(32).any(|bytes| bytes == secret);
            assert!(!copy, "a copy of the secret in {}", file.display());
        }

        let records = fs::read(dir.join("records.json")).unwrap();
        let again = offline.join("again");
        let named = args(&["--name", "n", "--out", again.to_str().unwrap()]);
        let err = command(&root, named).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let already = format!("node {node} has committed recovery key {key} already");
        assert!(err.to_string().starts_with(&already), "{err}");
        assert_eq!(fs::read(dir.join("records.json")).unwrap(), records);
        assert!(!again.exists(), "no second file");
    }

    /// The command writes the secret nowhere but a new file outside
    /// GLADE_HOME, named by an absolute path, and runs only on a stopped
    /// instance that has booted. Each refusal says why, and writes nothing:
    /// records.json is as it was, and no file appears.
    #[test]
    fn the_command_refuses_any_other_place_or_instance_and_writes_nothing() {
        let (root, offline) = fresh("refused");
        let dir = root.join("sys").join("n");
        drop(boot_at(dir.clone(), "gianni").unwrap());
        let records = fs::read(dir.join("records.json")).unwrap();
        let taken = offline.join("taken");
        fs::write(&taken, "an older file").unwrap();
        let text = |path: PathBuf| path.to_str().unwrap().to_owned();
        let out = |path: String| args(&["--name", "n", "--out", &path]);
        use io::ErrorKind::{AddrInUse, AlreadyExists, InvalidInput, NotFound};
        let cases = [
            (
                out(text(root.join("n.recovery"))),
                InvalidInput,
                "inside GLADE_HOME",
            ),
            (
                out(text(dir.join("n.recovery"))),
                InvalidInput,
                "inside GLADE_HOME",
            ),
            (
                out("n.recovery".into()),
                InvalidInput,
                "not an absolute path",
            ),
            (
                out(text(taken.clone())),
                AlreadyExists,
                "never written over it",
            ),
            (
                out(text(offline.join("no").join("n"))),
                NotFound,
                "cannot be used",
            ),
            (
                args(&["--name", "ghost", "--out", &text(offline.join("g"))]),
                NotFound,
                "no instance ghost",
            ),
            (
                args(&["--name", "n"]),
                InvalidInput,
                "usage: glade-node recovery",
            ),
            (
                args(&["--out", &text(offline.join("o"))]),
                InvalidInput,
                "usage",
            ),
        ];
        for (named, kind, why) in cases {
            let err = command(&root, named.clone()).unwrap_err();
            assert_eq!(err.kind(), kind, "{named:?}: {err}");
            assert!(err.to_string().contains(why), "{named:?}: {err}");
        }
        let running = boot_at(dir.clone(), "gianni").unwrap();
        let err = command(&root, out(text(offline.join("r")))).unwrap_err();
        assert_eq!(err.kind(), AddrInUse);
        assert!(err.to_string().ends_with("stop the node first"), "{err}");
        drop(running);

        assert_eq!(fs::read(dir.join("records.json")).unwrap(), records);
        assert_eq!(fs::read(&taken).unwrap(), b"an older file");
        let names: Vec<_> = fs::read_dir(&offline).unwrap().collect();
        assert_eq!(names.len(), 1, "only the older file: {names:?}");
    }

    /// F9: the command takes only an instance in `<root>/sys`, named as a
    /// boot names one. `../../beside/n`, which climbs out of a root that
    /// holds instances to one booted beside it, is refused before anything
    /// is read or written, and so are `..` and `.`: that instance's
    /// records.json is as it was, and no secret is written. Before the check,
    /// the command booted it and committed a key.
    #[test]
    fn the_command_refuses_a_name_outside_sys_and_writes_nothing() {
        let (root, offline) = fresh("named-outside");
        fs::create_dir_all(root.join("sys")).unwrap();
        let beside = root.parent().unwrap().join("beside").join("n");
        drop(boot_at(beside.clone(), "gianni").unwrap());
        let records = fs::read(beside.join("records.json")).unwrap();
        let out = offline.join("n.recovery");
        for name in ["../../beside/n", "..", "."] {
            let named = args(&["--name", name, "--out", out.to_str().unwrap()]);
            let err = command(&root, named).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{name}: {err}");
            let said = format!(
                "--name {name:?}: an instance name must match [A-Za-z0-9._-]{{1,63}} and be neither . nor .."
            );
            assert_eq!(err.to_string(), said);
        }
        assert_eq!(fs::read(beside.join("records.json")).unwrap(), records);
        assert!(!out.exists(), "no secret written");
    }

    /// `--recovery-out` at a first boot: the key is committed in the save
    /// that writes the node's presence, and its secret written. A later boot
    /// given one is refused before records.json is written, writing no file;
    /// a first boot given a path inside GLADE_HOME is refused before anything
    /// is written.
    #[test]
    fn a_first_boot_takes_recovery_out_and_a_later_boot_is_refused_it() {
        let (root, offline) = fresh("first-boot");
        let dir = root.join("sys").join("n");
        let inside = root.join("n.recovery");
        let err = boot_at_with(dir.clone(), "gianni", Some(&inside), LEASE_TTL_MS).map(|_| ());
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(!dir.exists(), "nothing written");

        let out = offline.join("n.recovery");
        let boot = boot_at_with(dir.clone(), "gianni", Some(&out), LEASE_TTL_MS).unwrap();
        let (_, key) = secret_of(&out);
        let file = fs::canonicalize(&out).unwrap();
        let committed = Some(Committed {
            key: key.clone(),
            file,
        });
        assert_eq!(boot.recovery, committed);
        drop(boot);
        let saved = boot_at(dir.clone(), "gianni").unwrap();
        assert_eq!(saved.registry.recovery_key(&saved.node_id), Some(key));
        drop(saved);

        let records = fs::read(dir.join("records.json")).unwrap();
        let again = offline.join("again");
        let err = boot_at_with(dir.clone(), "gianni", Some(&again), LEASE_TTL_MS).map(|_| ());
        let err = err.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let first_only = "--recovery-out is taken at a node's first boot only";
        assert!(err.to_string().starts_with(first_only), "{err}");
        assert_eq!(fs::read(dir.join("records.json")).unwrap(), records);
        assert!(!again.exists(), "no file written");
    }

    // File modes are a Unix notion. A braced module, so the condition
    // encloses the whole section.
    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// The secret is written mode 0600, as `node.key` is.
        #[test]
        fn the_secret_is_written_0600() {
            let (_, offline) = fresh("mode");
            let out = offline.join("n.recovery");
            mint("n", &out).unwrap();
            let mode = fs::metadata(&out).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
