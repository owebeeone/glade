//! Plan Step 3.3: the assembled path's start-up and cleanup as one sdax plan,
//! [`node_plan`]. `glade-node` starts it when run with `GLADE_NODE_ASSEMBLED=1`
//! and waits on it where the hand-written root waits on `server.run`. The design,
//! with the release graph and where each spawned task goes, is
//! `glade/dev-docs/GladeNodeAssembly.md`, "Lifecycle (plan Step 3.3)".
//!
//! ```text
//! Instance        resource
//! Assembly        step       needs Instance
//! Storage         resource   needs Instance, Assembly
//! PeerCarrier     resource   needs Instance, Storage
//! ClientCarrier   resource   needs Storage
//! Records         service    needs Storage, PeerCarrier
//! Sessions        service    needs Storage, PeerCarrier, ClientCarrier
//! Peers           step       needs Storage, Sessions
//! Workspaces      step       needs Assembly, Storage, Peers, Records
//! Listening       step       needs Sessions, ClientCarrier, Workspaces
//! ```
//!
//! (Each body also reads the run's input.) Cleanup is the reverse of those
//! `needs`, and independent cleanups may overlap. Four of the edges are the
//! partial order of `arch1/InjectionGraphRefinement.md:42-45`: `Records` drains
//! before `Storage` and `PeerCarrier` are released, `Sessions` before
//! `PeerCarrier` and `ClientCarrier`. `Records` and `Sessions` own every task
//! the node spawns (`tasks.rs`): at its stop an owner closes admission, cancels
//! its tasks and joins them, and only then are the resources they used
//! released. Each resource that the release gives up is taken out of its node
//! by value, so what is left in the engine's slots owns nothing.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use sdax::prelude::*;
use shaku::HasComponent;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::appdecl::AppDecl;
use crate::assembly::{
    CommandLine, Config, Directory, InstanceSlot, NodeAssembly, Records, Settings,
};
use crate::iroh_carrier::{PeerAddr, PeerEndpoint};
use crate::mesh::{release_links, EndpointSlot};
use crate::peer::NodeIdentity;
use crate::registry::HOME;
use crate::server::{accept_clients, Server, Shared};
use crate::sysdir::{boot_at, instance_dir, Boot, Profile};
use crate::tasks::{self, Inbox};

/// The node names, in one place, so a test and the plan cannot disagree
/// about a spelling: `ReleaseOrder::before` answers `false` for a typo.
pub mod node {
    /// The booted instance (nothing in the legacy form).
    pub const INSTANCE: &str = "Instance";
    /// Plan Step 3.2's Shaku module over the acquired instance.
    pub const ASSEMBLY: &str = "Assembly";
    /// The served store, with the adopted instance inside it.
    pub const STORAGE: &str = "Storage";
    /// The iroh endpoint (nothing in the legacy form).
    pub const PEER_CARRIER: &str = "PeerCarrier";
    /// The TCP listener the WebSocket sessions arrive on.
    pub const CLIENT_CARRIER: &str = "ClientCarrier";
    /// Owner of the renewal loop and the pushes of minted records.
    pub const RECORDS: &str = "Records";
    /// Owner of the links, streams, subscriptions and client sessions.
    pub const SESSIONS: &str = "Sessions";
    /// The `--peer` dials.
    pub const PEERS: &str = "Peers";
    /// The declared workspaces, served.
    pub const WORKSPACES: &str = "Workspaces";
    /// `listening <port>`, then clients are admitted.
    pub const LISTENING: &str = "Listening";
}

/// The settle-and-cleanup budget. Generous: iroh's endpoint close alone can
/// take about three seconds on a bad link.
pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(30);

/// How long an owner may take to cancel and join its tasks.
pub const STOP_WITHIN: Duration = Duration::from_secs(10);

/// Where the node's lines go: stdout and stderr for the binary, a list for a
/// test that reads them.
pub trait Console: Send + Sync {
    /// A line the hand-written root prints to stdout.
    fn out(&self, line: &str);
    /// A line it prints to stderr.
    fn err(&self, line: &str);
}

/// The binary's console. Stdout is flushed after every line, as the
/// `listening` line must reach a parent process that waits for it.
pub struct StdConsole;

impl Console for StdConsole {
    fn out(&self, line: &str) {
        println!("{line}");
        let _ = io::stdout().flush();
    }

    fn err(&self, line: &str) {
        eprintln!("{line}");
    }
}

/// Where the booted form's instance lives, and the operator a first boot
/// records.
pub struct InstanceAt {
    pub dir: PathBuf,
    pub operator: String,
}

/// One run's input: the parsed command line, the app files already loaded
/// (every one loads before anything is written), the instance to boot, if
/// any, and where lines go.
pub struct NodeStart {
    pub settings: Settings,
    pub decls: Vec<AppDecl>,
    pub instance: Option<InstanceAt>,
    pub console: Arc<dyn Console>,
}

impl NodeStart {
    /// As the binary starts: the booted form (`--profile` or `--name`) boots
    /// where `sysdir::boot` would, under `GLADE_HOME`.
    pub fn from_settings(
        settings: Settings,
        decls: Vec<AppDecl>,
        console: Arc<dyn Console>,
    ) -> NodeStart {
        let instance = settings.booted().then(|| InstanceAt {
            dir: instance_dir(
                settings.profile.unwrap_or(Profile::Local),
                settings.name.as_deref(),
            ),
            operator: settings.operator.clone().unwrap_or_else(|| "local".into()),
        });
        NodeStart {
            settings,
            decls,
            instance,
            console,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `Instance`: the booted instance, lent through Step 3.2's slot until
/// `Storage` adopts it by value.
struct Instance {
    slot: InstanceSlot,
    booted: Option<Booted>,
}

/// What the booted form reads from its instance before lending it.
struct Booted {
    dir: PathBuf,
    node_id: String,
    identity: NodeIdentity,
}

impl Instance {
    fn none() -> Instance {
        Instance {
            slot: Arc::new(Mutex::new(None)),
            booted: None,
        }
    }

    fn boot(start: &NodeStart, at: &InstanceAt) -> io::Result<Instance> {
        let boot = boot_at(at.dir.clone(), &at.operator)?;
        let booted = Booted {
            dir: boot.dir.clone(),
            node_id: boot.node_id.clone(),
            identity: boot.identity()?,
        };
        start
            .console
            .out(&format!("instance {}", boot.dir.display()));
        start.console.out(&format!("node {}", boot.node_id));
        if boot.rejected > 0 {
            start
                .console
                .out(&format!("quarantined {} record(s) at load", boot.rejected));
        }
        Ok(Instance {
            slot: Arc::new(Mutex::new(Some(boot))),
            booted: Some(booted),
        })
    }

    /// The instance, by value, if no one has adopted it.
    fn take(&self) -> Option<Boot> {
        lock(&self.slot).take()
    }
}

/// `Storage`: the served store, with the adopted instance inside it, and the
/// two owners' inboxes until their services take them.
struct Storage {
    server: Mutex<Option<Server>>,
    sessions: Mutex<Option<Inbox>>,
    records: Mutex<Option<Inbox>>,
}

impl Storage {
    /// Open the store, give its tasks to the two owners, adopt the instance.
    async fn open(start: &NodeStart, instance: &Instance) -> io::Result<Storage> {
        let dir = match (start.settings.store_dir(), &instance.booted) {
            (Some(dir), _) => PathBuf::from(dir),
            (None, Some(booted)) => booted.dir.join("cache").join("store"),
            (None, None) => std::env::temp_dir().join("glade-node-bin"),
        };
        let server = Server::open(&dir)?;
        let (owners, sessions, records) = tasks::owners();
        server.own_tasks(owners)?;
        if let Some(boot) = instance.take() {
            server.adopt_boot(boot).await?;
        }
        Ok(Storage {
            server: Mutex::new(Some(server)),
            sessions: Mutex::new(Some(sessions)),
            records: Mutex::new(Some(records)),
        })
    }

    /// A second handle on the served node, for one body.
    fn server(&self) -> Result<Server, Error> {
        let held = lock(&self.server);
        let shared = held.as_ref().map(|server| server.shared.clone());
        shared
            .map(|shared| Server { shared })
            .ok_or_else(|| "the node's storage has been released".into())
    }

    fn shared(&self) -> Result<Arc<Shared>, Error> {
        self.server().map(|server| server.shared)
    }

    fn inbox(&self, owner: &Mutex<Option<Inbox>>) -> Result<Inbox, Error> {
        lock(owner)
            .take()
            .ok_or_else(|| "an owner's inbox was taken twice".into())
    }

    /// Give the node state up by value. It must be the last owner: another
    /// is a task or a handle that outlived its owner, and would hold what the
    /// state holds (the instance lock, link handles) where the report cannot
    /// see it, so it is a cleanup failure here instead.
    fn release(&self) -> Result<(), Error> {
        drop(lock(&self.sessions).take());
        drop(lock(&self.records).take());
        let Some(server) = lock(&self.server).take() else {
            return Ok(());
        };
        let others = Arc::strong_count(&server.shared) - 1;
        drop(server);
        if others == 0 {
            return Ok(());
        }
        Err(format!("{others} other owner(s) of the node state outlived their owners").into())
    }
}

/// `ClientCarrier`: the bound listener, and its port.
struct Listener {
    listener: tokio::sync::Mutex<Option<TcpListener>>,
    port: u16,
}

impl Listener {
    async fn bind(port: u16) -> io::Result<Listener> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        Ok(Listener {
            listener: tokio::sync::Mutex::new(Some(listener)),
            port,
        })
    }
}

/// `Records`' handle: its inbox, until its serve body takes it.
struct RecordsOwner(Mutex<Option<Inbox>>);

/// The workspaces the app files declare, `(share, name)`: `Assembly`'s output.
type Declared = Vec<(String, String)>;

/// What `Workspaces` needs: the declared list and the store to serve it
/// from, once the peers are dialed and `Records` owns the renewals.
type WorkspacesNeeds = (
    Arc<NodeStart>,
    Arc<Declared>,
    Arc<Storage>,
    Arc<()>,
    Arc<RecordsOwner>,
);

/// `Sessions`' handle: what its serve body takes, and the gate `Listening`
/// opens to admit clients.
struct SessionsOwner {
    serve: Mutex<Option<SessionsServe>>,
    admit: watch::Sender<bool>,
}

struct SessionsServe {
    inbox: Inbox,
    storage: Arc<Storage>,
    listener: Arc<Listener>,
    admitted: watch::Receiver<bool>,
}

/// Plan Step 3.2's module, assembled over the acquired instance inside a
/// step: registers each app file through the directory and returns the
/// workspaces they declare. The module is dropped here; its record host
/// answers `NotOpen` once `Storage` has adopted the instance.
fn assemble(start: &NodeStart, instance: &Instance) -> Result<Declared, Error> {
    let module = NodeAssembly::builder()
        .with_component_parameters::<CommandLine>(start.settings.clone())
        .with_component_parameters::<Records>(instance.slot.clone())
        .build();
    let config: Arc<dyn Config> = module.resolve();
    let directory: Arc<dyn Directory> = module.resolve();
    let mut workspaces = Vec::new();
    let Some(booted) = &instance.booted else {
        return Ok(workspaces);
    };
    let serves_home = directory.serves(HOME).map_err(io::Error::from)?.is_some();
    start
        .console
        .out(&format!("registry ready (home served: {serves_home})"));
    for (path, decl) in config.settings().apps.iter().zip(&start.decls) {
        // The non-fatal channel (R10(a)), as the hand-written root prints it.
        for line in decl.warning_lines(path) {
            start.console.err(&line);
        }
        let reg = directory
            .register(decl, &booted.node_id)
            .map_err(io::Error::from)?;
        let (app, appended, unchanged) = (&decl.app, reg.appended, reg.unchanged);
        start.console.out(&format!(
            "app {app} registered (+{appended} record(s), {unchanged} unchanged)"
        ));
        workspaces.extend(
            decl.workspaces
                .iter()
                .map(|w| (w.share.clone(), w.name.clone())),
        );
    }
    Ok(workspaces)
}

/// Dial each `--peer` target, as the hand-written root does. The legacy form
/// has no mesh and dials nothing.
async fn dial_peers(start: &NodeStart, server: &Server) {
    if start.instance.is_none() {
        return;
    }
    for p in &start.settings.peers {
        match PeerAddr::parse(p) {
            Some(target) => match server.connect_peer(&target).await {
                Ok(id) => start.console.out(&format!("peer-connected {id}")),
                Err(e) => start.console.err(&format!("peer {p}: {e}")),
            },
            None => start
                .console
                .err(&format!("peer {p}: expected <endpoint-id>@<ip:port>")),
        }
    }
}

/// `Sessions`' serve body: the owner loop, and the client accept loop once
/// `Listening` admits clients, until its stop. Then the stop: admission
/// closed and every task cancelled and joined, and only then every link
/// closed by value, when no task is left to register one.
async fn serve_sessions(cx: Cx<ServingPhase>, owner: Arc<SessionsOwner>) -> Result<(), Error> {
    let Some(serve) = lock(&owner.serve).take() else {
        return Ok(());
    };
    let SessionsServe {
        mut inbox,
        storage,
        listener,
        mut admitted,
    } = serve;
    let shared = storage.shared()?;
    let mut tasks = JoinSet::new();
    let clients = async {
        if admitted.wait_for(|admit| *admit).await.is_err() {
            std::future::pending::<()>().await;
        }
        let held = listener.listener.lock().await;
        match held.as_ref() {
            Some(listener) => accept_clients(&shared, listener).await,
            None => std::future::pending().await,
        }
    };
    let ended = tokio::select! {
        () = cx.stop() => Ok(()),
        failed = clients => failed,
        () = tasks::own(&mut inbox, &mut tasks) => Ok(()),
    };
    tasks::finish(inbox, tasks).await;
    release_links(&shared).await;
    Ok(ended?)
}

/// `Records`' serve body: the owner loop until its stop, then admission
/// closed and every task cancelled and joined.
async fn serve_records(cx: Cx<ServingPhase>, owner: Arc<RecordsOwner>) -> Result<(), Error> {
    let Some(mut inbox) = lock(&owner.0).take() else {
        return Ok(());
    };
    let mut tasks = JoinSet::new();
    tokio::select! {
        () = cx.stop() => {}
        () = tasks::own(&mut inbox, &mut tasks) => {}
    }
    tasks::finish(inbox, tasks).await;
    Ok(())
}

/// The node's plan: Resident, fail-fast, [`SHUTDOWN_BUDGET`] to settle and
/// clean up. Building it runs no body; each `start` is one node.
pub fn node_plan() -> Plan<(), NodeStart> {
    let mut p = Plan::with_input::<NodeStart>("GladeNode");
    let start = p.input();

    let instance = p
        .resource(node::INSTANCE)
        .needs(start)
        .acquire(|cx: Cx<Acquire>, start: Arc<NodeStart>| async move {
            if start.instance.is_none() {
                return Ok(cx.hold_value(Instance::none()));
            }
            cx.hold(move || async move {
                let at = start.instance.as_ref().expect("checked above");
                Instance::boot(&start, at)
            })
            .await
        })
        // A start that failed before `Storage` adopted the instance still
        // releases its lock here; after adoption there is nothing left.
        .release(|_cx: Cx<Release>, instance: Arc<Instance>| async move {
            drop(instance.take());
            Ok(())
        });

    let assembly = p.step(node::ASSEMBLY).needs((start, instance)).run(
        |_cx: Cx<Run>, (start, instance): (Arc<NodeStart>, Arc<Instance>)| async move {
            assemble(&start, &instance)
        },
    );

    let storage = p
        .resource(node::STORAGE)
        .needs((start, instance, assembly))
        .acquire(
            |cx: Cx<Acquire>,
             (start, instance, _registered): (
                Arc<NodeStart>,
                Arc<Instance>,
                Arc<Declared>,
            )| async move {
                cx.hold(move || async move { Storage::open(&start, &instance).await })
                    .await
            },
        )
        .release(|_cx: Cx<Release>, storage: Arc<Storage>| async move { storage.release() });

    let peer_carrier = p
        .resource(node::PEER_CARRIER)
        .needs((instance, storage))
        .acquire(
            |cx: Cx<Acquire>, (instance, _storage): (Arc<Instance>, Arc<Storage>)| async move {
                let Some(identity) = instance.booted.as_ref().map(|booted| booted.identity) else {
                    return Ok(cx.hold_value(EndpointSlot::empty()));
                };
                cx.hold(move || async move {
                    PeerEndpoint::bind_with(identity)
                        .await
                        .map(EndpointSlot::new)
                })
                .await
            },
        )
        // `PeerEndpoint::close(self)`, on the endpoint taken out by value: the
        // mesh shares this slot, so it holds nothing afterwards.
        .release(|_cx: Cx<Release>, slot: Arc<EndpointSlot>| async move {
            if let Some(endpoint) = slot.give_up() {
                endpoint.close().await;
            }
            Ok(())
        });

    let client_carrier = p
        .resource(node::CLIENT_CARRIER)
        .needs((start, storage))
        .acquire(
            |cx: Cx<Acquire>, (start, _storage): (Arc<NodeStart>, Arc<Storage>)| async move {
                let port = start.settings.port();
                cx.hold(move || async move { Listener::bind(port).await })
                    .await
            },
        )
        .release(|_cx: Cx<Release>, listener: Arc<Listener>| async move {
            drop(listener.listener.lock().await.take());
            Ok(())
        });

    let records = p
        .service(node::RECORDS)
        .needs((storage, peer_carrier))
        .stop_within(STOP_WITHIN)
        .initialize(
            |_cx: Cx<Start>, (storage, _peer): (Arc<Storage>, Arc<EndpointSlot>)| async move {
                let inbox = storage.inbox(&storage.records)?;
                Ok(RecordsOwner(Mutex::new(Some(inbox))))
            },
        )
        .serve(serve_records);

    let sessions = p
        .service(node::SESSIONS)
        .needs((start, storage, peer_carrier, client_carrier))
        .stop_within(STOP_WITHIN)
        .initialize(
            |_cx: Cx<Start>,
             (start, storage, peer, listener): (
                Arc<NodeStart>,
                Arc<Storage>,
                Arc<EndpointSlot>,
                Arc<Listener>,
            )| async move {
                let inbox = storage.inbox(&storage.sessions)?;
                if start.instance.is_some() {
                    let addr = storage
                        .server()?
                        .enable_mesh_over(EndpointSlot::clone(&peer))
                        .await?;
                    start
                        .console
                        .out(&format!("peer {} {}", addr.endpoint_id, addr.socket));
                }
                let (admit, admitted) = watch::channel(false);
                let serve = SessionsServe {
                    inbox,
                    storage,
                    listener,
                    admitted,
                };
                Ok(SessionsOwner {
                    serve: Mutex::new(Some(serve)),
                    admit,
                })
            },
        )
        .serve(serve_sessions);

    let peers = p
        .step(node::PEERS)
        .needs((start, storage, sessions))
        .run(|_cx: Cx<Run>, (start, storage, _sessions): (Arc<NodeStart>, Arc<Storage>, Arc<SessionsOwner>)| async move {
            dial_peers(&start, &storage.server()?).await;
            Ok(())
        });

    let workspaces = p
        .step(node::WORKSPACES)
        .needs((start, assembly, storage, peers, records))
        .run(
            |_cx: Cx<Run>, (start, declared, storage, _peers, _records): WorkspacesNeeds| async move {
                let server = storage.server()?;
                for (share, name) in declared.iter() {
                    server.serve_workspace(share, name).await?;
                    start.console.out(&format!("workspace {share} serving"));
                }
                Ok(())
            },
        );

    p.step(node::LISTENING)
        .needs((start, sessions, client_carrier, workspaces))
        .run(
            |_cx: Cx<Run>,
             (start, sessions, listener, _served): (
                Arc<NodeStart>,
                Arc<SessionsOwner>,
                Arc<Listener>,
                Arc<()>,
            )| async move {
                start.console.out(&format!("listening {}", listener.port));
                sessions.admit.send_replace(true);
                Ok(())
            },
        );

    p.build(
        Policy::FailFast,
        Shutdown::within(SHUTDOWN_BUDGET),
        Mode::Resident,
    )
    .expect("the node plan is valid by construction")
}

/// Say how a finished run ended, and whether it stopped clean: the binary
/// exits 0 when it did and 1 when it did not. Clean is `report.is_clean()`:
/// outcome `Ok`, and no fault, cleanup failure, `incomplete` or `ambiguous`
/// record. Each fault's message goes to stderr as the hand-written root
/// prints a failed start; anything else unclean prints the report.
pub fn conclude(report: &Report, console: &dyn Console) -> bool {
    if report.is_clean() {
        return true;
    }
    for fault in &report.faults {
        console.err(&fault.kind.to_string());
    }
    let more = !report.cleanup_failures.is_empty()
        || !report.incomplete.is_empty()
        || !report.ambiguous.is_empty()
        || report.faults.is_empty();
    if more {
        for line in report.to_string().lines() {
            console.err(line);
        }
    }
    false
}

// Unit tests of what the integration tests cannot reach. A braced module, so
// the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;

    /// The release check on `Storage` fails closed: a node state with another
    /// owner is refused as a cleanup failure, not dropped in silence.
    #[test]
    fn storage_refuses_to_release_a_state_that_something_else_still_owns() {
        let dir =
            std::env::temp_dir().join(format!("glade-lifecycle-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let server = Server::open(&dir).unwrap();
        let escaped = server.shared.clone();
        let storage = Storage {
            server: Mutex::new(Some(server)),
            sessions: Mutex::new(None),
            records: Mutex::new(None),
        };
        let refused = storage.release().expect_err("an escaped owner is refused");
        assert_eq!(
            refused.to_string(),
            "1 other owner(s) of the node state outlived their owners"
        );
        assert_eq!(
            Arc::strong_count(&escaped),
            1,
            "the release gave its own handle up"
        );
        assert!(
            storage.release().is_ok(),
            "and a second release has nothing left"
        );
        drop(escaped);
        // `Server::open` writes nothing until something is appended.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
