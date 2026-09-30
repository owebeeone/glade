//! `glade-node decode-dry-run DIR...`: the dry run of TautCheckedDecode.md
//! §7 (CD-G3 item 2), before a node's binary is replaced by one on taut's
//! fail-closed codec. It decodes every op and every `home` record a node's
//! data holds with that codec, as the node reads them, and counts what the
//! codec refuses. Strict-canonical decode may refuse bytes the legacy codec
//! took, and the node then quarantines such an op in records.json at load
//! (which leaves its grant fold unreadable), skips it in a journal, and
//! skips such a record in each fold that reads it (F15b).
//!
//! Under each DIR, walked without following a link, it reads each
//! `records.json`, a node's snapshot, and each journal of a served store:
//! `<hex share>/<hex origin>.log` and `proofs/equivocations.log`. It reads
//! nothing else, a key never. It writes nothing and takes no lock, so it may
//! run beside the node whose data it reads: a journal's torn tail, which the
//! store cuts at its next open, is noted and left.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use glade_wire::cbor::{self, Cbor, DecodeError};
use glade_wire::generated::Op;

use crate::envelope;
use crate::records_file;
use crate::registry::{
    G_BINDINGS, G_BINDING_RETRACTIONS, G_CLAIMS, G_GRANTS, G_NODES, G_PRINCIPALS, G_RECOVERY_KEYS,
    G_REVOCATIONS, G_SERVICES, G_WORKSPACES, HOME,
};
use crate::sysdata::{
    BindingDecl, BindingRetraction, CapabilityGrant, CapabilityRevocation, NodeRecord,
    NodeRecoveryKey, PrincipalRecord, ServeClaim, ServiceDefinition, WorkspaceEntry,
};

/// What a dry run said, one line each, and how much the codec refused.
#[derive(Debug, Default)]
pub struct DryRun {
    pub lines: Vec<String>,
    /// The ops and records refused, and the files that cannot be read.
    pub refused: usize,
}

/// Run `glade-node decode-dry-run` over the directories `args` names. A
/// directory that cannot be walked, or that holds neither a snapshot nor a
/// journal, is an error, so a wrong path is never a clean run.
pub fn command(args: impl IntoIterator<Item = String>) -> io::Result<DryRun> {
    let dirs: Vec<PathBuf> = args.into_iter().map(PathBuf::from).collect();
    if dirs.is_empty() {
        let usage = "usage: glade-node decode-dry-run DIR...";
        return Err(io::Error::new(io::ErrorKind::InvalidInput, usage));
    }
    let mut found = Vec::new();
    for dir in &dirs {
        let before = found.len();
        let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", dir.display()));
        fs::metadata(dir).map_err(named)?;
        walk(dir, &mut found).map_err(named)?;
        if found.len() == before {
            let none = format!(
                "found no records.json and no journal under {}",
                dir.display()
            );
            return Err(io::Error::new(io::ErrorKind::NotFound, none));
        }
    }
    let mut dry = DryRun::default();
    let (mut ops, mut records) = (0, 0);
    for (path, file) in &found {
        let read = match fs::read(path) {
            Ok(bytes) => file.read(&bytes),
            Err(e) => Err(format!("cannot be read ({e})")),
        };
        match read {
            Ok(read) => {
                (ops, records) = (ops + read.ops, records + read.records);
                dry.refused += read.refused.len();
                dry.lines
                    .push(format!("{}: {}", path.display(), read.summary()));
                dry.lines
                    .extend(read.refused.into_iter().map(|line| format!("  {line}")));
            }
            Err(why) => {
                dry.refused += 1;
                dry.lines.push(format!("{}: {why}", path.display()));
            }
        }
    }
    let (files, ops) = (count(found.len(), "file"), count(ops, "op"));
    let (records, refused) = (count(records, "home record"), refused(dry.refused));
    dry.lines.push(format!(
        "decode dry run: {files}, {ops} and {records} read, {refused}"
    ));
    Ok(dry)
}

/// What the node reads a file as.
#[derive(Clone, Copy, Debug, PartialEq)]
enum File {
    /// records.json, the node's snapshot.
    Snapshot,
    /// A served store's journal of framed ops.
    Journal,
}

impl File {
    /// What the node reads a file at `path` as, by its name, if anything.
    fn at(path: &Path) -> Option<File> {
        let name = path.file_name()?.to_str()?;
        if name == "records.json" {
            return Some(File::Snapshot);
        }
        let dir = path.parent()?.file_name()?.to_str()?;
        let op_log = name.strip_suffix(".log").is_some_and(hex) && hex(dir);
        let proofs = dir == "proofs" && name == "equivocations.log";
        (op_log || proofs).then_some(File::Journal)
    }

    /// `bytes` read as this file, or why the node could not start on them.
    fn read(self, bytes: &[u8]) -> Result<Read, String> {
        match self {
            File::Snapshot => snapshot(bytes).map_err(|why| {
                format!("cannot be read as a snapshot ({why}); the node refuses to start on it")
            }),
            File::Journal => Ok(journal(bytes)),
        }
    }
}

/// Whether `name` is a name the store gives: lower-case hex, two digits a byte.
fn hex(name: &str) -> bool {
    let digits = name
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    !name.is_empty() && name.len().is_multiple_of(2) && digits
}

/// Every file under `dir` the node reads as a snapshot or a journal, in the
/// order of their paths; a link is not followed.
fn walk(dir: &Path, found: &mut Vec<(PathBuf, File)>) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let (path, kind) = (entry.path(), entry.file_type()?);
        if kind.is_dir() {
            walk(&path, found)?;
        } else if kind.is_file() {
            if let Some(file) = File::at(&path) {
                found.push((path, file));
            }
        }
    }
    Ok(())
}

/// What one file held: the ops and `home` records read, a line for each the
/// codec refused, and a note on its torn tail.
#[derive(Default)]
struct Read {
    ops: usize,
    records: usize,
    refused: Vec<String>,
    tail: Option<String>,
}

impl Read {
    /// The file's line: its counts, and its torn tail.
    fn summary(&self) -> String {
        let (ops, records) = (count(self.ops, "op"), count(self.records, "home record"));
        let said = format!("{ops}, {records}, {}", refused(self.refused.len()));
        match &self.tail {
            Some(tail) => format!("{said}; {tail}"),
            None => said,
        }
    }

    /// The op at record `n` of the file, `bytes`, which the node, refusing
    /// it, `fate`s; then, on the `home` share, its record.
    fn op(&mut self, n: usize, bytes: &[u8], fate: &str) {
        self.ops += 1;
        let op = match envelope::decode_op(bytes) {
            Ok(op) => op,
            Err(why) => {
                self.refused
                    .push(format!("record {n}: op refused ({fate}): {}", said(&why)));
                return;
            }
        };
        if op.share != HOME {
            return;
        }
        self.records += 1;
        if let Err((kind, why)) = record(&op) {
            let (stream, origin, seq) = (&op.glade_id, &op.origin, op.seq);
            let at = format!("record {n} (home/{stream} of {origin}, seq {seq})");
            let line = format!(
                "{at}: record refused as {kind} (folds skip it): {}",
                said(&why)
            );
            self.refused.push(line);
        }
    }
}

/// records.json's ops, each as the registry's load reads it.
fn snapshot(bytes: &[u8]) -> Result<Read, &'static str> {
    let snap = records_file::decode(bytes)?;
    let mut read = Read::default();
    let fate = "quarantined at load; the grant fold is then unreadable";
    for (at, op) in snap.records.iter().enumerate() {
        read.op(at + 1, op, fate);
    }
    Ok(read)
}

/// A journal's ops, each as the store's `open` reads it: its length (`u32`,
/// little endian), then its bytes. A tail too short for its record is noted.
fn journal(bytes: &[u8]) -> Read {
    let mut read = Read::default();
    let (mut at, mut n) = (0, 0);
    while let Some(head) = bytes.get(at..at + 4) {
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(op) = (at + 4)
            .checked_add(len)
            .and_then(|end| bytes.get(at + 4..end))
        else {
            break;
        };
        n += 1;
        read.op(n, op, "skipped at open");
        at += 4 + len;
    }
    if at < bytes.len() {
        let torn = count(bytes.len() - at, "byte");
        let cut = "which the store cuts at its next open";
        read.tail = Some(format!("a torn tail of {torn} after record {n}, {cut}"));
    }
    read
}

/// The record a `home` op carries, read as the node reads it: as CBOR, and,
/// on a stream whose folds read it through the codec (`envelope::folded`,
/// `envelope::record`), as its stream's kind. The rest (checkpoints and
/// transport bindings) the node reads by shape, not through the codec.
fn record(op: &Op) -> Result<(), (&'static str, DecodeError)> {
    let bytes = envelope::record_bytes(&op.payload);
    let record = cbor::try_decode(&bytes).map_err(|why| ("CBOR", why))?;
    let (kind, read) = match kind(&op.glade_id, &record) {
        Some(kind) => kind,
        None => return Ok(()),
    };
    read.map_err(|why| (kind, why))
}

/// `record` read as the kind `glade_id`'s folds read it through the codec,
/// named, or `None` for a stream no fold reads that way.
fn kind(glade_id: &str, record: &Cbor) -> Option<(&'static str, Result<(), DecodeError>)> {
    let c = record;
    Some(match glade_id {
        G_NODES => ("NodeRecord", NodeRecord::from_cbor(c).map(drop)),
        G_WORKSPACES => ("WorkspaceEntry", WorkspaceEntry::from_cbor(c).map(drop)),
        G_CLAIMS => ("ServeClaim", ServeClaim::from_cbor(c).map(drop)),
        G_GRANTS => ("CapabilityGrant", CapabilityGrant::from_cbor(c).map(drop)),
        G_REVOCATIONS => (
            "CapabilityRevocation",
            CapabilityRevocation::from_cbor(c).map(drop),
        ),
        G_BINDINGS => ("BindingDecl", BindingDecl::from_cbor(c).map(drop)),
        G_SERVICES => (
            "ServiceDefinition",
            ServiceDefinition::from_cbor(c).map(drop),
        ),
        G_BINDING_RETRACTIONS => (
            "BindingRetraction",
            BindingRetraction::from_cbor(c).map(drop),
        ),
        G_PRINCIPALS => ("PrincipalRecord", PrincipalRecord::from_cbor(c).map(drop)),
        G_RECOVERY_KEYS => ("NodeRecoveryKey", NodeRecoveryKey::from_cbor(c).map(drop)),
        _ => return None,
    })
}

/// A refusal as a line says it: taut's words, then its canonical tag.
fn said(why: &DecodeError) -> String {
    format!("{why} ({})", why.tag())
}

/// `n` of `what`, in the plural but for one.
fn count(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        _ => format!("{n} {what}s"),
    }
}

/// How many were refused, or none.
fn refused(n: usize) -> String {
    match n {
        0 => "none refused".to_string(),
        _ => format!("{n} refused"),
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use glade_wire::cbor::{self, Cbor};
    use glade_wire::generated::Op;

    use super::*;
    use crate::envelope::testing::sealed;
    use crate::records_file;
    use crate::registry::{Record, G_CLAIMS, G_TRANSPORT_BINDINGS, HOME};
    use crate::sysdata::{NodeRecord, ServeClaim, SystemSnapshot};

    const SEED: [u8; 32] = [23; 32];

    /// A fresh directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-dry-run-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `records` as a node's records.json holds them.
    fn snapshot(records: Vec<Vec<u8>>) -> Vec<u8> {
        let snap = SystemSnapshot {
            records,
            heads: vec![],
            revision: None,
        };
        records_file::encode(&snap)
    }

    /// `records` framed as a journal holds them: each its length, then it.
    fn framed(records: &[Vec<u8>]) -> Vec<u8> {
        let frame = |bytes: &Vec<u8>| [&(bytes.len() as u32).to_le_bytes()[..], bytes].concat();
        records.iter().flat_map(frame).collect()
    }

    fn encoded(op: &Op) -> Vec<u8> {
        cbor::encode(&op.to_cbor())
    }

    /// An op of the `home` share on `glade_id`, carrying `record` unsealed.
    fn home(glade_id: &str, record: Vec<u8>) -> Op {
        Op {
            share: HOME.into(),
            glade_id: glade_id.into(),
            origin: "n1".into(),
            seq: 4,
            payload: record,
            ..Op::default()
        }
    }

    /// An app's op, its payload opaque to the node.
    fn app() -> Op {
        Op {
            share: "ws-a".into(),
            glade_id: "a.log".into(),
            origin: "o".into(),
            payload: vec![0xff, 0x00],
            ..Op::default()
        }
    }

    /// Hex, as the store names its journals.
    fn hex(text: &str) -> String {
        text.bytes().map(|b| format!("{b:02x}")).collect()
    }

    /// An instance's data: records.json at `dir`, the store's journals under
    /// `cache/store`, one per share and origin, and its proofs.
    fn instance(dir: &Path, records: Vec<Vec<u8>>, journals: &[(&str, &str, Vec<u8>)]) {
        fs::write(dir.join("records.json"), snapshot(records)).unwrap();
        for (share, origin, bytes) in journals {
            let at = match *share {
                "proofs" => dir.join("cache/store/proofs/equivocations.log"),
                _ => dir
                    .join("cache/store")
                    .join(hex(share))
                    .join(format!("{}.log", hex(origin))),
            };
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(at, bytes).unwrap();
        }
    }

    fn run(dirs: &[&Path]) -> DryRun {
        let args = dirs.iter().map(|dir| dir.display().to_string());
        command(args).unwrap()
    }

    /// Every op and every `home` record of a snapshot and of the store's
    /// journals that the codec takes is read and counted, and none refused:
    /// a line per file, sealed records of the kinds the folds read and an
    /// app's op, then the total. Exit 0.
    #[test]
    fn what_the_codec_takes_is_counted_and_nothing_is_refused() {
        let dir = scratch("clean");
        let node = sealed(
            SEED,
            Record::Node(NodeRecord {
                node_id: "n".into(),
                operator: "o".into(),
            }),
        );
        let claim = ServeClaim {
            node: "n".into(),
            share: "ws-a".into(),
            lease_expiry_ms: 9,
            epoch: 1,
        };
        let claim = sealed(SEED, Record::Serve(claim));
        let journals = [
            (HOME, "n", framed(&[encoded(&node), encoded(&claim)])),
            ("ws-a", "o", framed(&[encoded(&app()), encoded(&app())])),
            ("proofs", "", framed(&[encoded(&app()), encoded(&app())])),
        ];
        instance(&dir, vec![encoded(&node), encoded(&claim)], &journals);
        let dry = run(&[&dir]);
        let at = |file: &str| format!("{}", dir.join(file).display());
        let home = format!("cache/store/{}/{}.log", hex(HOME), hex("n"));
        let app = format!("cache/store/{}/{}.log", hex("ws-a"), hex("o"));
        let proofs = "cache/store/proofs/equivocations.log";
        assert_eq!(
            dry.lines,
            [
                format!("{}: 2 ops, 2 home records, none refused", at(&home)),
                format!("{}: 2 ops, 0 home records, none refused", at(&app)),
                format!("{}: 2 ops, 0 home records, none refused", at(proofs)),
                format!(
                    "{}: 2 ops, 2 home records, none refused",
                    at("records.json")
                ),
                "decode dry run: 4 files, 8 ops and 4 home records read, none refused".to_string(),
            ]
        );
        assert_eq!(dry.refused, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Each op the codec refuses is counted and named, with what the node
    /// does with it: nested too deep, of another shape, a non-canonical int.
    /// So is each `home` record it refuses: as CBOR (a non-canonical int), or
    /// as its stream's kind where a fold reads it through the codec (a field
    /// missing). A record on a stream no fold reads that way is read as CBOR
    /// only. A journal's torn tail, which the store cuts at its next open,
    /// is noted, not counted.
    #[test]
    fn each_op_or_record_the_codec_refuses_is_counted_and_named() {
        let dir = scratch("refused");
        let mut nested = vec![0x81; 100_000];
        nested.push(0);
        let not_an_op = cbor::encode(&Cbor::Map(vec![(1, Cbor::Text("home".into()))]));
        let non_canonical = home(G_CLAIMS, vec![0xa1, 0x18, 0x01, 0x00]);
        let two_texts = Cbor::Map(vec![
            (1, Cbor::Text("n".into())),
            (2, Cbor::Text("s".into())),
        ]);
        let short_claim = home(G_CLAIMS, cbor::encode(&two_texts));
        let binding = home(G_TRANSPORT_BINDINGS, cbor::encode(&two_texts));
        let records = vec![
            encoded(&app()),
            nested,
            not_an_op,
            encoded(&non_canonical),
            encoded(&short_claim),
            encoded(&binding),
        ];
        let torn = [
            framed(&[encoded(&app()), vec![0x18, 0x01]]),
            vec![0x10, 0, 0, 0, 0xaa],
        ]
        .concat();
        instance(&dir, records, &[("ws-a", "o", torn)]);
        let dry = run(&[&dir]);
        let journal = dir.join(format!("cache/store/{}/{}.log", hex("ws-a"), hex("o")));
        let records = dir.join("records.json");
        let (journal, records) = (journal.display(), records.display());
        let tail = "a torn tail of 5 bytes after record 2, which the store cuts at its next open";
        let quarantined = "op refused (quarantined at load; the grant fold is then unreadable)";
        let claim = "(home/dir.claims of n1, seq 4): record refused";
        assert_eq!(
            dry.lines,
            [
                format!("{journal}: 2 ops, 0 home records, 1 refused; {tail}"),
                "  record 2: op refused (skipped at open): non-canonical integer encoding of 1 (NonCanonicalInt)".to_string(),
                format!("{records}: 6 ops, 3 home records, 4 refused"),
                format!("  record 2: {quarantined}: CBOR nested deeper than 32 (TooDeep)"),
                format!("  record 3: {quarantined}: missing map key 2 (MissingKey)"),
                format!("  record 4 {claim} as CBOR (folds skip it): non-canonical integer encoding of 1 (NonCanonicalInt)"),
                format!("  record 5 {claim} as ServeClaim (folds skip it): missing map key 3 (MissingKey)"),
                "decode dry run: 2 files, 8 ops and 3 home records read, 5 refused".to_string(),
            ]
        );
        assert_eq!(dry.refused, 5);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// It reads records.json and the store's journals only, and changes
    /// nothing: a journal's torn tail stays, and a file the node never reads
    /// as a snapshot or a journal (a key, `local.json`, a snapshot set aside,
    /// a `.log` outside the store's layout) is not read, so bytes there that
    /// would not decode are not reported. A snapshot that cannot be read is
    /// counted: the node refuses to start on it.
    #[test]
    fn it_reads_the_snapshot_and_the_journals_alone_and_changes_nothing() {
        let dir = scratch("alone");
        let torn = [framed(&[encoded(&app())]), vec![0x10, 0, 0]].concat();
        instance(&dir, vec![encoded(&app())], &[("ws-a", "o", torn.clone())]);
        let garbage = [0xc0, 0x00];
        let others = [
            "node.key",
            "endpoint.key",
            "local.json",
            "records.legacy-2026-09-30.json",
            "grazel.log",
            "cache/store/notes/0a.log",
        ];
        for other in others {
            fs::create_dir_all(dir.join(other).parent().unwrap()).unwrap();
            fs::write(dir.join(other), garbage).unwrap();
        }
        let journal = dir.join(format!("cache/store/{}/{}.log", hex("ws-a"), hex("o")));
        let dry = run(&[&dir]);
        assert_eq!(dry.refused, 0, "{:?}", dry.lines);
        let total = "decode dry run: 2 files, 2 ops and 0 home records read, none refused";
        assert_eq!(dry.lines.last().map(String::as_str), Some(total));
        assert_eq!(fs::read(&journal).unwrap(), torn, "the torn tail stays");

        fs::write(dir.join("records.json"), garbage).unwrap();
        let dry = run(&[&dir]);
        let records = dir.join("records.json");
        let unreadable = format!(
            "{}: cannot be read as a snapshot (not a map); the node refuses to start on it",
            records.display()
        );
        assert!(dry.lines.contains(&unreadable), "{:?}", dry.lines);
        assert_eq!(dry.refused, 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    // Unix links, in a braced module, so the condition encloses the whole
    // section.
    #[cfg(unix)]
    mod links {
        use super::*;

        /// A link is not followed: a snapshot a linked directory holds is
        /// not read.
        #[test]
        fn a_link_is_not_followed() {
            let dir = scratch("linking");
            instance(&dir, vec![encoded(&app())], &[]);
            let elsewhere = scratch("linked");
            fs::write(elsewhere.join("records.json"), [0xc0, 0x00]).unwrap();
            std::os::unix::fs::symlink(&elsewhere, dir.join("linked")).unwrap();
            let dry = run(&[&dir]);
            let total = "decode dry run: 1 file, 1 op and 0 home records read, none refused";
            assert_eq!(dry.lines.last().map(String::as_str), Some(total));
            fs::remove_dir_all(&dir).unwrap();
            fs::remove_dir_all(&elsewhere).unwrap();
        }
    }

    /// Nothing to read is an error, not a clean run: no directory named, a
    /// directory that is not there, and one holding neither a snapshot nor a
    /// journal.
    #[test]
    fn nothing_to_read_is_an_error_not_a_clean_run() {
        let usage = command(Vec::new()).unwrap_err();
        assert_eq!(usage.to_string(), "usage: glade-node decode-dry-run DIR...");
        let empty = scratch("empty");
        let missing = empty.join("missing");
        let absent = command([missing.display().to_string()]).unwrap_err();
        assert_eq!(absent.kind(), io::ErrorKind::NotFound);
        assert!(
            absent
                .to_string()
                .starts_with(&missing.display().to_string()),
            "{absent}"
        );
        let nothing = command([empty.display().to_string()]).unwrap_err();
        let said = format!(
            "found no records.json and no journal under {}",
            empty.display()
        );
        assert_eq!(nothing.to_string(), said);
        fs::remove_dir_all(&empty).unwrap();
    }
}
