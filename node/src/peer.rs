//! Node<->node session (Lane R step 2): the HELLO seam + heads/gap sync.
//!
//! Carrier-free by construction — everything here runs over any `AsyncRead +
//! AsyncWrite` pair, so the protocol is unit-tested over an in-memory duplex and
//! rides real iroh QUIC (`iroh_carrier.rs`) unchanged. Two layers:
//!
//!   1. **Framed IO** — a `u32`-length prefix around each `Frame` (the exact
//!      framing the WS carrier uses, minus the websocket).
//!   2. **HELLO seam** — peers exchange a node identity (`node_id =
//!      sha256(node key)`, the stubbed-but-structure-real posture from
//!      GladeSystemDataSeamNotes; ed25519 swaps in behind `verify_peer`). The
//!      s-sync DIAL gate: operator chains "verify", but NOTHING downstream trusts
//!      this — sync integrity is end-to-end from origin chains, never the carrier.
//!
//! The heads/gap sync driver (per-(origin, zone) chains, verify-as-ingest,
//! reject-suffix + re-fetch, equivocation proof) lives in the second half.

use std::collections::BTreeMap;
use std::io;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use glade_wire::generated::{Heads, NodeHello, NodeWelcome, Op, Ops, Priority};

use crate::frame::Frame;
use crate::session::missing_for;
use crate::store::{EquivProof, Store, StoreError};

/// Wire protocol version spoken on the peer link.
pub const PROTOCOL: i64 = 1;

// ---- framed IO ------------------------------------------------------------

/// Write one frame, length-prefixed (`u32` LE) then flushed.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &Frame) -> io::Result<()> {
    let bytes = frame.to_bytes();
    w.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame. A clean stream close at a frame boundary
/// surfaces as `UnexpectedEof` — the sync loop reads that as "peer done".
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let n = u32::from_le_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Frame::from_bytes(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ---- node identity + HELLO seam ------------------------------------------

/// A node's stubbed-but-structure-real identity: `node_id = sha256(key)`
/// (GladeSystemDataSeamNotes). The `key` is a 32-byte node key — today an
/// arbitrary seed (the iroh secret-key public bytes in the carrier); ed25519
/// swaps in behind the same shape without a wire change.
#[derive(Clone, Copy, Debug)]
pub struct NodeIdentity {
    pub key: [u8; 32],
    pub node_id: [u8; 32],
}

impl NodeIdentity {
    /// Derive the identity from a node key: `node_id = sha256(key)`.
    pub fn from_key(key: [u8; 32]) -> Self {
        NodeIdentity { key, node_id: Sha256::digest(key).into() }
    }

    /// The origin/operator signature seam over the handshake. STUBBED: a
    /// domain-separated digest, not a real signature — `verify_peer` accepts
    /// unconditionally today. Real ed25519 over `node_id` drops in here.
    fn stub_sig(&self) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update(b"glade/peer/hello");
        h.update(self.key);
        h.finalize().to_vec()
    }
}

/// The verified peer, as far as the (stubbed) seam vouches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeerHello {
    pub peer_id: [u8; 32],
}

fn peer_id_of(node_id: &[u8]) -> io::Result<[u8; 32]> {
    node_id
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "peer node_id not 32 bytes"))
}

/// The HELLO verification seam. STUBBED: structure is real (we parse the claimed
/// node_id and carry the signature) but the check always accepts — matching the
/// s-sync gate note that sync integrity never depends on this handshake.
fn verify_peer(node_id: &[u8], _sig: &Option<Vec<u8>>) -> io::Result<PeerHello> {
    Ok(PeerHello { peer_id: peer_id_of(node_id)? })
}

/// Dialer side of the node<->node HELLO: send `NodeHello`, await `NodeWelcome`,
/// return the (stubbed-)verified peer identity.
pub async fn hello_dial<R, W>(r: &mut R, w: &mut W, me: &NodeIdentity) -> io::Result<PeerHello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let hello = NodeHello { node_id: me.node_id.to_vec(), protocol: PROTOCOL, sig: Some(me.stub_sig()) };
    write_frame(w, &Frame::NodeHello(hello)).await?;
    match read_frame(r).await? {
        Frame::NodeWelcome(nw) => verify_peer(&nw.node_id, &nw.sig),
        other => Err(io::Error::new(io::ErrorKind::InvalidData, format!("expected NodeWelcome, got {other:?}"))),
    }
}

/// Acceptor side: await `NodeHello`, reply `NodeWelcome`, return the peer.
pub async fn hello_accept<R, W>(r: &mut R, w: &mut W, me: &NodeIdentity) -> io::Result<PeerHello>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let peer = match read_frame(r).await? {
        Frame::NodeHello(nh) => verify_peer(&nh.node_id, &nh.sig)?,
        other => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("expected NodeHello, got {other:?}"))),
    };
    let welcome = NodeWelcome { node_id: me.node_id.to_vec(), protocol: PROTOCOL, sig: Some(me.stub_sig()) };
    write_frame(w, &Frame::NodeWelcome(welcome)).await?;
    Ok(peer)
}

// ---- heads/gap sync -------------------------------------------------------

/// Max ops per streamed chunk. Bulk backfill is size-capped so it never
/// head-of-line-blocks interactive traffic — the §6 scheduler guarantee applied
/// to sync. Resume is free: the receiver's HEADS advance as ops land.
pub const OPS_PER_CHUNK: usize = 64;

/// Per-op origin-signature verification seam (GQ-9). STUBBED: called before an
/// op can land, but currently accepts. Real ed25519 over the op's canonical
/// bytes drops in here — closing the one-op tamper window (GladePeerSyncNotes §6).
pub fn verify_origin_sig(_op: &Op) -> bool {
    true
}

/// A `(share, glade_id, key, origin)` chain — the per-(origin, zone) unit (D8).
pub type ChainKey = (String, String, Vec<u8>, String);

fn chain_key(op: &Op) -> ChainKey {
    (op.share.clone(), op.glade_id.clone(), op.key.clone(), op.origin.clone())
}

/// What a pull produced.
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Ops that landed (appended or idempotent duplicate).
    pub applied: usize,
    /// (origin, zone) chains whose suffix was rejected FROM THIS PEER — chain
    /// break, gap, or bad signature. Nothing that peer sent after the break is
    /// kept; the caller re-fetches these chains elsewhere (resume is exact).
    pub rejected: Vec<ChainKey>,
    /// Equivocation proofs newly recorded while ingesting this stream — a signed
    /// fork by the ORIGIN (SY4), not the carrier's fault.
    pub equivocations: Vec<EquivProof>,
}

/// Server side of a pull (the s-sync responder): read the peer's HEADS, stream
/// exactly the ops it lacks for every zone we hold, in size-capped BULK chunks,
/// then close the write half — that close is the "gap complete" terminator.
///
/// Offers every zone this store holds. ACL zone-filtering (a peer withholding an
/// entire private-zone chain) is a drop-in here: filter `store.zones()` — the
/// per-(origin, zone) shape means absent chains are absences, not holes.
pub async fn serve_sync<R, W>(r: &mut R, w: &mut W, store: &Store) -> io::Result<usize>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let their = match read_frame(r).await? {
        Frame::Heads(h) => h.streams,
        other => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("expected Heads, got {other:?}"))),
    };
    // Index the peer's vectors by zone -> origin -> seq.
    let mut their_by_zone: BTreeMap<(String, String, Vec<u8>), BTreeMap<String, i64>> = BTreeMap::new();
    for sh in their {
        let m = their_by_zone.entry((sh.share.clone(), sh.glade_id.clone(), sh.key.clone())).or_default();
        for hd in sh.heads {
            m.insert(hd.origin, hd.seq);
        }
    }
    let mut sent = 0usize;
    for (share, glade_id, key) in store.zones() {
        let their_v = their_by_zone.get(&(share.clone(), glade_id.clone(), key.clone())).cloned().unwrap_or_default();
        let gap = missing_for(store, &share, &glade_id, &key, &their_v);
        for chunk in gap.chunks(OPS_PER_CHUNK) {
            write_frame(w, &Frame::Ops(Ops { ops: chunk.to_vec(), pri: Some(Priority::Bulk) })).await?;
            sent += chunk.len();
        }
    }
    w.shutdown().await?; // close = gap complete
    Ok(sent)
}

/// Dialer side of a pull (the s-sync initiator): announce our HEADS, then ingest
/// the peer's gap stream, VERIFYING EACH OP AS IT LANDS (`store.append` =
/// prev-hash continuity + seq monotonic + equivocation, plus the origin-sig
/// seam). On a chain-check failure the whole suffix of that (origin, zone) chain
/// from this peer is dropped and reported for re-fetch; equivocation records a
/// proof. Ends at the peer's stream close.
pub async fn pull_sync<R, W>(r: &mut R, w: &mut W, store: &mut Store) -> io::Result<SyncOutcome>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    write_frame(w, &Frame::Heads(Heads { streams: store.all_heads() })).await?;
    let before = store.equivocation_proofs().len();
    let mut out = SyncOutcome::default();
    loop {
        let frame = match read_frame(r).await {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break, // peer closed = done
            Err(e) => return Err(e),
        };
        let Frame::Ops(ops) = frame else { continue }; // pull channel carries only Ops
        for op in ops.ops {
            let ck = chain_key(&op);
            if out.rejected.contains(&ck) {
                continue; // suffix of an already-broken chain from this peer
            }
            if !verify_origin_sig(&op) {
                out.rejected.push(ck);
                continue;
            }
            match store.append(op) {
                Ok(_) => out.applied += 1,
                Err(StoreError::ChainBreak { .. }) | Err(StoreError::Gap { .. }) => out.rejected.push(ck),
                Err(StoreError::Equivocation { .. }) => {} // proof recorded in the store
                Err(StoreError::Io(e)) => return Err(e),
            }
        }
    }
    out.equivocations = store.equivocation_proofs()[before..].to_vec();
    Ok(out)
}

#[cfg(test)]
mod hello_tests {
    use super::*;
    use tokio::io::split;

    /// node_id is sha256(key) — deterministic, and distinct keys give distinct ids.
    #[test]
    fn node_id_is_sha256_of_key() {
        let a = NodeIdentity::from_key([1u8; 32]);
        let b = NodeIdentity::from_key([1u8; 32]);
        let c = NodeIdentity::from_key([2u8; 32]);
        assert_eq!(a.node_id, b.node_id); // deterministic
        assert_ne!(a.node_id, c.node_id); // key-bound
        assert_eq!(a.node_id.to_vec(), sha2::Sha256::digest([1u8; 32]).to_vec());
    }

    /// The DIAL gate over an in-memory duplex: dialer and acceptor complete the
    /// HELLO and each learns the OTHER's node_id (not its own).
    #[tokio::test]
    async fn hello_handshake_exchanges_identities() {
        let dialer = NodeIdentity::from_key([7u8; 32]);
        let acceptor = NodeIdentity::from_key([9u8; 32]);

        // duplex(a,b): writing a is readable on b. Split each end into (r, w).
        let (a, b) = tokio::io::duplex(4096);
        let (mut ar, mut aw) = split(a);
        let (mut br, mut bw) = split(b);

        let acc = tokio::spawn(async move { hello_accept(&mut br, &mut bw, &acceptor).await });
        let seen_by_dialer = hello_dial(&mut ar, &mut aw, &dialer).await.unwrap();
        let seen_by_acceptor = acc.await.unwrap().unwrap();

        assert_eq!(seen_by_dialer.peer_id, acceptor.node_id, "dialer learns acceptor id");
        assert_eq!(seen_by_acceptor.peer_id, dialer.node_id, "acceptor learns dialer id");
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use glade_wire::generated::Shape;
    use std::path::PathBuf;
    use tokio::io::split;

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glade-peer-sync-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn op(origin: &str, seq: i64, key: &[u8], prev: Option<Vec<u8>>, payload: &[u8]) -> Op {
        Op {
            share: "sh".into(),
            glade_id: "g".into(),
            key: key.to_vec(),
            origin: origin.into(),
            seq,
            prev,
            lamport: seq,
            refs: vec![],
            shape: Shape::Value,
            payload: payload.to_vec(),
        }
    }

    /// Append a valid prev-linked chain of `n` ops for `origin` in `key`'s zone,
    /// returning the ops (so a test can replay/tamper them as a carrier would).
    fn chained(store: &mut Store, origin: &str, key: &[u8], n: i64) -> Vec<Op> {
        let mut prev = None;
        let mut ops = Vec::new();
        for seq in 0..n {
            let o = op(origin, seq, key, prev.clone(), format!("{origin}{seq}").as_bytes());
            store.append(o.clone()).unwrap();
            prev = Some(crate::chain::op_hash(&o).to_vec());
            ops.push(o);
        }
        ops
    }

    /// SY1+SY2: a fresh replica pulls the exact gap over a duplex and verifies
    /// every op as it lands — two chains, both converge byte-for-byte.
    #[tokio::test]
    async fn pull_converges_and_verifies() {
        let mut server = Store::open(fresh("srv")).unwrap();
        chained(&mut server, "a", b"", 5);
        chained(&mut server, "b", b"", 3);
        let mut client = Store::open(fresh("cli")).unwrap();

        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca); // client end
        let (mut br, mut bw) = split(cb); // server end

        let srv = tokio::spawn(async move { serve_sync(&mut br, &mut bw, &server).await });
        let out = pull_sync(&mut ar, &mut aw, &mut client).await.unwrap();
        let sent = srv.await.unwrap().unwrap();

        assert_eq!(sent, 8);
        assert_eq!(out.applied, 8);
        assert!(out.rejected.is_empty());
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 5);
        assert_eq!(client.scan("sh", "g", b"", "b", -1).len(), 3);
    }

    /// SY3: a tampering carrier flips op 3's `prev`. The chain check rejects op 3
    /// and the whole suffix FROM THIS PEER; re-fetching the chain from an honest
    /// replica resumes exactly (the retry costs one range, not one share).
    #[tokio::test]
    async fn tampered_suffix_rejected_then_refetched() {
        let mut truth = Store::open(fresh("truth")).unwrap();
        let ops = chained(&mut truth, "a", b"", 5);
        let mut client = Store::open(fresh("tamper-cli")).unwrap();

        // Malicious peer: ops 0,1,2 valid, op 3 with a tampered prev, then op 4.
        let mut bad3 = ops[3].clone();
        bad3.prev = Some(vec![0u8; 32]); // != hash(op2) -> chain break at seq 3
        let malicious = vec![ops[0].clone(), ops[1].clone(), ops[2].clone(), bad3, ops[4].clone()];

        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca);
        let (mut br, mut bw) = split(cb);
        let carrier = tokio::spawn(async move {
            read_frame(&mut br).await.unwrap(); // client HEADS
            write_frame(&mut bw, &Frame::Ops(Ops { ops: malicious, pri: Some(Priority::Bulk) })).await.unwrap();
            bw.shutdown().await.unwrap();
        });
        let out = pull_sync(&mut ar, &mut aw, &mut client).await.unwrap();
        carrier.await.unwrap();

        assert_eq!(out.applied, 3); // 0,1,2 landed; 3 broke, 4 (suffix) dropped
        assert_eq!(out.rejected, vec![("sh".into(), "g".into(), vec![], "a".into())]);
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 3);

        // Re-fetch from the honest replica: resume from head 2, gain 3 and 4.
        let (ca2, cb2) = tokio::io::duplex(64 * 1024);
        let (mut ar2, mut aw2) = split(ca2);
        let (mut br2, mut bw2) = split(cb2);
        let srv = tokio::spawn(async move { serve_sync(&mut br2, &mut bw2, &truth).await });
        let out2 = pull_sync(&mut ar2, &mut aw2, &mut client).await.unwrap();
        srv.await.unwrap().unwrap();

        assert!(out2.rejected.is_empty());
        assert_eq!(out2.applied, 2);
        assert_eq!(client.scan("sh", "g", b"", "a", -1).len(), 5); // full chain restored
    }

    /// SY4: the peer streams a second signed op into a slot the client already
    /// holds — an origin fork. Ingest rejects it and surfaces the proof.
    #[tokio::test]
    async fn pull_surfaces_equivocation_proof() {
        let mut client = Store::open(fresh("equiv")).unwrap();
        client.append(op("a", 0, b"", None, b"A")).unwrap();
        let conflict = op("a", 0, b"", None, b"B"); // same slot, different hash

        let (ca, cb) = tokio::io::duplex(64 * 1024);
        let (mut ar, mut aw) = split(ca);
        let (mut br, mut bw) = split(cb);
        let carrier = tokio::spawn(async move {
            read_frame(&mut br).await.unwrap();
            write_frame(&mut bw, &Frame::Ops(Ops { ops: vec![conflict], pri: Some(Priority::Bulk) })).await.unwrap();
            bw.shutdown().await.unwrap();
        });
        let out = pull_sync(&mut ar, &mut aw, &mut client).await.unwrap();
        carrier.await.unwrap();

        assert_eq!(out.applied, 0);
        assert_eq!(out.equivocations.len(), 1);
        assert_eq!(out.equivocations[0].a.payload, b"A");
        assert_eq!(out.equivocations[0].b.payload, b"B");
        assert_eq!(client.equivocation_proofs().len(), 1); // persisted in the store
    }
}
