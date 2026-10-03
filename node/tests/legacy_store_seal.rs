//! Q4-A compiling consumers: a legacy-store interlock, never a Raft receipt.
mod common {
    use std::fs;
    use std::path::PathBuf;

    use glade_node::store::{Append, Store, StoreError};
    use glade_wire::generated::{Op, Shape};

    pub(super) const MARKER: &str = "legacy-store.sealed";

    pub(super) struct Fixture(pub(super) PathBuf);

    impl Fixture {
        pub(super) fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("glade-q4-seal-{}-{label}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub(super) fn store(&self) -> Store {
            Store::open(&self.0).unwrap()
        }

        pub(super) fn journal(&self) -> Vec<u8> {
            fs::read(self.0.join("77732d72617a656c").join("7461622d61.log")).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    pub(super) fn value(seq: i64, payload: &[u8]) -> Op {
        Op {
            share: "ws-razel".into(),
            glade_id: "gyld.appearance".into(),
            key: b"self:alice".to_vec(),
            origin: "tab-a".into(),
            seq,
            prev: None,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }

    pub(super) fn refused(result: Result<Append, StoreError>) {
        assert!(
            matches!(result, Err(StoreError::Io(ref e))
        if e.kind() == std::io::ErrorKind::PermissionDenied),
            "{result:?}"
        );
    }

    #[test]
    fn ls001_unsealed_private_value_and_exact_duplicate_survive_reopen() {
        let f = Fixture::new("unsealed");
        let mut store = f.store();
        let op = value(0, b"complete appearance value");
        assert_eq!(store.append(op.clone()).unwrap(), Append::Appended);
        assert_eq!(store.append(op.clone()).unwrap(), Append::Duplicate);
        drop(store);
        let mut reopened = f.store();
        assert_eq!(reopened.append(op.clone()).unwrap(), Append::Duplicate);
        assert_eq!(
            reopened.scan(&op.share, &op.glade_id, &op.key, &op.origin, -1),
            [op]
        );
    }
    #[test]
    fn ls004_unknown_marker_bytes_are_not_absence() {
        let f = Fixture::new("unknown-marker");
        let mut store = f.store();
        store.append(value(0, b"before")).unwrap();
        let before = f.journal();
        fs::write(f.0.join(MARKER), b"unrecognized interrupted publication").unwrap();
        refused(store.append(value(1, b"after")));
        assert!(Store::open(&f.0).is_err());
        assert_eq!(f.journal(), before);
    }
    #[test]
    fn ls004_directory_marker_and_lock_io_error_fail_closed() {
        let f = Fixture::new("directory-marker");
        let mut store = f.store();
        fs::create_dir(f.0.join(MARKER)).unwrap();
        refused(store.append(value(0, b"blocked")));
        assert!(Store::open(&f.0).is_err());

        let bad = Fixture::new("lock-io");
        let mut store = bad.store();
        let lock = bad.0.join("legacy-store.lock");
        if lock.exists() {
            fs::remove_file(&lock).unwrap();
        }
        fs::create_dir(&lock).unwrap();
        assert!(store.append(value(0, b"blocked")).is_err());
        assert!(store.seal_legacy().is_err());
        assert!(Store::open(&bad.0).is_err());
    }
    #[test]
    fn ls005_refused_open_never_repairs_a_torn_legacy_journal() {
        let f = Fixture::new("no-repair");
        let mut store = f.store();
        store.append(value(0, b"complete")).unwrap();
        let path = f.0.join("77732d72617a656c").join("7461622d61.log");
        let mut torn = f.journal();
        torn.extend_from_slice(&[40, 0, 0, 0, 0x81]);
        fs::write(&path, &torn).unwrap();
        fs::write(f.0.join(MARKER), []).unwrap();
        drop(store);
        assert!(
            Store::open(&f.0).is_err(),
            "sealed open must fail before replay"
        );
        assert_eq!(fs::read(path).unwrap(), torn);
    }

    #[test]
    fn ls004_empty_marker_closes_new_duplicate_and_fork_before_proof_writes() {
        let f = Fixture::new("empty-marker");
        let mut store = f.store();
        let original = value(0, b"original");
        store.append(original.clone()).unwrap();
        let before = f.journal();
        fs::write(f.0.join(MARKER), []).unwrap();
        refused(store.append(original));
        refused(store.append(value(0, b"fork")));
        refused(store.append(value(1, b"next")));
        assert!(Store::open(&f.0).is_err());
        assert!(!f.0.join("proofs").exists());
        assert_eq!(f.journal(), before);
    }
}

#[cfg(unix)]
mod unix_profile {
    use super::common::*;
    use glade_node::server::Server;
    use glade_node::store::{Append, Store};
    #[test]
    fn ls002_success_seals_both_existing_handles_and_shared_server_reopen() {
        let f = Fixture::new("seal-success");
        let mut first = f.store();
        let original = value(0, b"preexisting complete value");
        first.append(original.clone()).unwrap();
        let mut other = f.store();
        let before = f.journal();
        first.seal_legacy().expect("LS-002 successful durable seal");
        refused(first.append(value(1, b"new")));
        refused(other.append(original.clone()));
        refused(other.append(value(0, b"fork")));
        assert_eq!(f.journal(), before);
        assert!(!f.0.join("proofs").exists());
        assert!(Store::open(&f.0).is_err());
        assert!(Server::open(&f.0).is_err());
        // Cached reads have no freshness/readiness claim, but no data is erased.
        assert_eq!(
            first.scan(
                &original.share,
                &original.glade_id,
                &original.key,
                &original.origin,
                -1
            ),
            [original]
        );
        first.seal_legacy().expect("idempotent monotonic retry");
    }
    #[test]
    fn ls007_sealing_one_root_does_not_enroll_or_stop_another() {
        let selected = Fixture::new("selected");
        let separate = Fixture::new("separate");
        let mut store = selected.store();
        store.seal_legacy().expect("selected seal");
        let mut unaffected = separate.store();
        assert_eq!(
            unaffected.append(value(0, b"independent")).unwrap(),
            Append::Appended
        );
        assert!(!separate.0.join(MARKER).exists());
    }
    #[test]
    fn ls004_dangling_marker_is_present_and_never_followed() {
        let f = Fixture::new("dangling");
        let mut store = f.store();
        std::os::unix::fs::symlink("missing", f.0.join(MARKER)).unwrap();
        refused(store.append(value(0, b"blocked")));
        assert!(Store::open(&f.0).is_err());
        assert!(store.seal_legacy().is_err());
        assert!(!f.0.join("missing").exists());
    }
}

#[cfg(not(unix))]
mod unsupported_profile {
    #[test]
    fn sealing_refuses_without_publishing_on_unqualified_platform() {
        let path =
            std::env::temp_dir().join(format!("glade-seal-unsupported-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let mut store = glade_node::store::Store::open(&path).unwrap();
        assert!(
            matches!(store.seal_legacy(), Err(glade_node::store::StoreError::Io(ref e)) if e.kind() == std::io::ErrorKind::Unsupported)
        );
        assert!(!path.join("legacy-store.sealed").exists());
        std::fs::remove_dir_all(path).unwrap();
    }
}
