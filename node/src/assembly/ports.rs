use std::fmt;
use std::io;

use glade_carrier_api::{CarrierAddr, CarrierError, PortFuture, TransportId};
use glade_wire::generated::Op;

use crate::appdecl::{AppDecl, Registered};
use crate::registry::{Record, RegistryError};

// ---- the node-local ports -------------------------------------------------

/// `directory_profile_binding`'s port: what the directory profile hosts, which
/// the record host applies to every op it is handed. Pure. Node-local.
pub trait RecordProfilePort: Send + Sync {
    /// The share every record of this profile lives on.
    fn share(&self) -> &str;

    /// Whether `glade_id` is one of this profile's record streams.
    fn hosts(&self, glade_id: &str) -> bool;
}

/// `directory_host_binding`'s port: the record host the directory facade reads
/// and writes. Node-local: it names the node's own record types.
pub trait RecordHostPort: Send + Sync {
    /// Append `record` as `origin`'s next op and persist the fold. A
    /// byte-identical record already held is not appended again: `Ok(false)`,
    /// the exact-retry rule.
    fn append(&self, record: Record, origin: &str) -> Result<bool, HostError>;

    /// Register an app file's declarations under `origin`
    /// (`appdecl::register`), then persist the fold.
    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError>;

    /// Take in an op carried from elsewhere (a peer, the disk): refused unless
    /// the profile hosts its share and stream, then verified as it lands. An
    /// op already held, byte for byte, is a duplicate: `Ok`, saving nothing.
    fn ingest(&self, op: Op) -> Result<(), HostError>;

    /// The node serving `share` at the reader's instant `now_ms`; lease expiry
    /// is judged at read time, never folded.
    fn who_serves(&self, share: &str, now_ms: i64) -> Result<Option<String>, HostError>;
}

/// `record_transport_binding`'s port: how the record host reaches a peer.
/// Node-local.
pub trait TransportPort: Send + Sync {
    /// Push `records`, each an encoded op, to `peer` over one link, best
    /// effort: a frame per record, then the link closes. Resolves with how many
    /// frames the carrier took, which is not an acknowledgement.
    fn push<'a>(
        &'a self,
        peer: &'a CarrierAddr,
        records: &'a [Vec<u8>],
    ) -> PortFuture<'a, Result<usize, CarrierError>>;
}

/// What a peer carrier can note beyond the carrier port (plan Step 4.5b, the
/// owner's ruling of 2026-09-27): the path the newest live link to an endpoint
/// sends on, and the home relays' states, for the node's `link` and `relay`
/// lines. Node-local: status lines are not the carrier contract's, and no
/// transport's own types cross it.
pub trait LinkNotes: Send + Sync {
    /// The path the newest live link to `remote` sends on, if the carrier has
    /// selected one; none for an endpoint no live link reaches.
    fn path(&self, remote: &TransportId) -> Option<PathSeen>;

    /// Hand `seen` the home relays' states, now and at each change, until the
    /// port closes. A port bound with no relays, or not bound, has none to
    /// report, and the watch ends at once. It holds no handle on the endpoint.
    fn relay_watch(&self, seen: Box<dyn FnMut(Vec<RelayState>) + Send>) -> PortFuture<'static, ()>;
}

/// The path a link sends on, as its `link` line reads it: where it goes,
/// `relay <url>` or `direct <ip:port>`, and the carrier's round-trip estimate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSeen {
    pub via: String,
    pub rtt_ms: u128,
}

/// A home relay's state, as the `relay` lines read it: its URL as the node
/// prints it, whether the endpoint is connected to it, and while it is not,
/// the last error, if one has been seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayState {
    pub url: String,
    pub connected: bool,
    pub error: Option<String>,
}

/// Why the record host refused.
#[derive(Debug)]
pub enum HostError {
    /// The host holds no instance: none was lent (the legacy form boots none),
    /// or the Server has adopted it.
    NotOpen,
    /// An op outside the profile: another share, or a stream it does not host.
    OutOfScope { share: String, glade_id: String },
    /// The registry's verify-as-ingest refused the record.
    Rejected(RegistryError),
    /// Persisting the fold failed.
    Io(io::Error),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::NotOpen => write!(f, "the record host holds no instance"),
            HostError::OutOfScope { share, glade_id } => {
                write!(f, "{share}/{glade_id} is outside the directory profile")
            }
            HostError::Rejected(e) => write!(f, "the registry refused the record: {e:?}"),
            HostError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HostError {}

/// A failed save, as `Registry::accept` reports it.
impl From<io::Error> for HostError {
    fn from(e: io::Error) -> HostError {
        HostError::Io(e)
    }
}

/// As the hand-written root reports the same failures: a refused registration
/// as `InvalidData` with the registry's `Debug` text, a failed write as the
/// I/O error itself.
impl From<HostError> for io::Error {
    fn from(e: HostError) -> io::Error {
        match e {
            HostError::Io(e) => e,
            HostError::Rejected(e) => io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")),
            other => io::Error::other(other.to_string()),
        }
    }
}
