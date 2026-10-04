//! Explicit disposable node provisioning; never inferred from discovery or legacy stores.
use super::*;
use crate::assembly::NodeAssembly;
use crate::iroh_carrier::IrohCarrier;
use glade_crdt_recovery_api::{LoadResult, ReplicaRecoverySession};
use glade_crdt_recovery_codec::*;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod exchange;
pub(crate) use exchange::serve;

const VERSION: &[u8] = b"glade/ic3/node-qualification/v1";
fn bounds() -> DecodeLimits {
    DecodeLimits {
        bytes: 4 * 1024 * 1024,
        items: 65536,
        depth: 64,
        name_bytes: 512,
    }
}
fn invalid(why: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}
fn field<T: Canonical>(fields: &[Bytes], index: usize) -> io::Result<T> {
    decode(fields[index].value(), &bounds()).map_err(|error| invalid(format!("{error:?}")))
}
pub(crate) struct Provision {
    pub request: ReplicaOpen,
    pub policy: SignedRecord,
    pub certificates: Vec<SignedRecord>,
    pub node_seed: Digest,
    pub endpoint_seed: Digest,
    pub data: PathBuf,
    pub trusted_floor: PathBuf,
    pub registration: Digest,
    pub fresh: bool,
    pub time: Option<TimeInterval>,
    pub time_floor_ms: u64,
    pub peers: Vec<(Digest, (Digest, String))>,
    pub tick_ms: u64,
    pub deadline_ms: u64,
}
impl Provision {
    pub fn read(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() || !std::fs::symlink_metadata(path)?.is_file() {
            return Err(invalid(
                "independent provisioning must be an absolute regular file",
            ));
        }
        let mut raw = vec![];
        File::open(path)?
            .take(bounds().bytes as u64 + 1)
            .read_to_end(&mut raw)?;
        let fields: Vec<Bytes> =
            decode(&raw, &bounds()).map_err(|error| invalid(format!("{error:?}")))?;
        if fields.len() != 12 || fields[0].value() != VERSION {
            return Err(invalid("closed independent provisioning version/fields"));
        }
        let roots: Vec<String> = field(&fields, 6)?;
        if roots.len() != 2 || roots.iter().any(|root| !Path::new(root).is_absolute()) {
            return Err(invalid(
                "separate absolute owned data and trusted floor roots required",
            ));
        }
        let (time, time_floor_ms) = field(&fields, 9)?;
        let (tick_ms, deadline_ms): (u64, u64) = field(&fields, 11)?;
        if tick_ms == 0 || tick_ms > 60_000 || deadline_ms == 0 || deadline_ms > 60_000 {
            return Err(invalid(
                "finite nonzero qualification scheduling bounds required",
            ));
        }
        let seed = |index: usize| {
            fields[index]
                .value()
                .try_into()
                .map_err(|_| invalid("exact seed/digest width"))
        };
        let node_seed = seed(4)?;
        let endpoint_seed = seed(5)?;
        let peers: Vec<(Digest, (Digest, String))> = field(&fields, 10)?;
        let mut nodes = std::collections::BTreeSet::new();
        let mut endpoints = std::collections::BTreeSet::new();
        if peers.len() > 16
            || peers.iter().any(|(node, (endpoint, address))| {
                *node == [0; 32]
                    || *endpoint == [0; 32]
                    || address.len() > 512
                    || *node == crate::signing::public_key(&node_seed)
                    || *endpoint == crate::signing::public_key(&endpoint_seed)
                    || !nodes.insert(*node)
                    || !endpoints.insert(*endpoint)
                    || crate::iroh_carrier::entry_of(&glade_carrier_api::CarrierAddr(
                        address.clone(),
                    ))
                    .is_none_or(|entry| entry.key != *endpoint || entry.via.is_empty())
            })
        {
            return Err(invalid(
                "bounded explicit peer identities/endpoints required",
            ));
        }
        Ok(Self {
            request: field(&fields, 1)?,
            policy: field(&fields, 2)?,
            certificates: field(&fields, 3)?,
            node_seed,
            endpoint_seed,
            data: roots[0].clone().into(),
            trusted_floor: roots[1].clone().into(),
            registration: seed(7)?,
            fresh: field(&fields, 8)?,
            time,
            time_floor_ms,
            peers,
            tick_ms,
            deadline_ms,
        })
    }
}
pub(crate) struct CurrentSource {
    cut: TrustedCut,
}
impl PolicyClock for CurrentSource {
    fn current(&mut self) -> Result<TrustedCut, Fault> {
        Ok(self.cut.clone())
    }
}
/// One actual node scope owns its module, real disk session and carrier. Each
/// resource instance has an independent session/root; no shared root CAS.
pub(crate) struct IndependentReplica {
    pub provision: Arc<Provision>,
    pub carrier: IrohCarrier,
    pub session: Mutex<Option<DiskSession<CurrentSource>>>,
    pub exchange_gate: tokio::sync::Mutex<()>,
    pub exchange_active: Mutex<Option<(Digest, Arc<dyn glade_carrier_api::CarrierLink>)>>,
    _module: Arc<NodeAssembly>,
}
impl IndependentReplica {
    pub fn open(
        module: Arc<NodeAssembly>,
        provision: Arc<Provision>,
        carrier: IrohCarrier,
    ) -> io::Result<Self> {
        let config = EvidenceConfig {
            descriptor: provision.request.descriptor.clone(),
            identity: provision.request.identity.clone(),
            trusted_root: provision.request.identity.root,
            node_seed: provision.node_seed,
            limits: provision.request.limits.decode.clone(),
            certificates: provision.certificates.clone(),
            session_capacity: provision.request.limits.guard_history,
        };
        let source = CurrentSource {
            cut: TrustedCut {
                policy: provision.policy.clone(),
                time: provision.time.clone(),
                time_floor_ms: provision.time_floor_ms,
            },
        };
        let roots = DiskRootConfig {
            data: provision.data.clone(),
            trusted_floor: provision.trusted_floor.clone(),
            registration: provision.registration,
            provision: provision.fresh,
        };
        let mut adapters = module
            .independent_configured(config, source, roots)
            .map_err(|error| invalid(format!("{error:?}")))?;
        let mut session = adapters
            .recovery
            .open_node(provision.request.clone())
            .map_err(|error| invalid(format!("{error:?}")))?;
        if !matches!(session.load(), LoadResult::Validated(_)) {
            return Err(invalid("independent replica recovery unavailable"));
        }
        session
            .resume_node_inputs()
            .map_err(|error| invalid(format!("independent inbox recovery: {error:?}")))?;
        Ok(Self {
            provision,
            carrier,
            session: Mutex::new(Some(session)),
            exchange_gate: tokio::sync::Mutex::new(()),
            exchange_active: Mutex::new(None),
            _module: module,
        })
    }
    pub fn close(&self) -> io::Result<()> {
        let mut held = self
            .session
            .lock()
            .map_err(|_| invalid("independent session poisoned"))?;
        if let Some(session) = held.as_mut() {
            if !matches!(ReplicaRecoverySession::close(session), CloseResult::Closed) {
                return Err(invalid(
                    "independent receive/storage custody remains pending",
                ));
            }
        }
        held.take();
        Ok(())
    }
}

pub(crate) async fn read_frame(
    stream: &mut tokio::net::TcpStream,
    bound: usize,
) -> Result<Bytes, Fault> {
    use tokio::io::AsyncReadExt;
    let size = stream.read_u32_le().await.map_err(|_| Fault::Unavailable)? as usize;
    if size > bound {
        return Err(Fault::Capacity);
    }
    let mut raw = vec![0; size];
    stream
        .read_exact(&mut raw)
        .await
        .map_err(|_| Fault::Unavailable)?;
    Ok(Bytes::new(raw))
}
pub(crate) async fn write_frame(
    stream: &mut tokio::net::TcpStream,
    bytes: &Bytes,
) -> Result<(), Fault> {
    use tokio::io::AsyncWriteExt;
    let size = u32::try_from(bytes.value().len()).map_err(|_| Fault::Capacity)?;
    stream
        .write_u32_le(size)
        .await
        .map_err(|_| Fault::Unavailable)?;
    stream
        .write_all(bytes.value())
        .await
        .map_err(|_| Fault::Unavailable)
}
