//! `glade-node [PORT] [STORE_DIR]` — run a glade node (GLP-0005 + GDL-036).
//!
//! Two invocation forms:
//!
//! **Legacy serve form** (no flags): `glade-node <port> [store_dir]` — the
//! pre-seam contract, byte-for-byte: serve the app-data carrier from
//! `store_dir` (default: a temp dir), NO sysdir boot, NO `~/.glade` access.
//! The grip-share integration suite spawns this form (concurrently — a global
//! singleton lock here would collide, and tests must never write $HOME).
//!
//! **Booted profile form** (opt-in): `glade-node --profile local|peer|server
//! [--name NAME] [--operator OP] [--app FILE.glade]... [--peer ID[@IP:PORT]]...
//! [PORT] [STORE_DIR]` —
//! reads every `--app` file, then boots the system-data instance (GDL-036): acquires
//! `~/.glade/sys/<name>/` (the profile picks the default name; `--name`
//! overrides; `GLADE_HOME` overrides `$HOME/.glade`), runs the load-validation
//! ladder (the first boot after plan Step 4.1a also sets aside, once, the
//! records naming the node by its key's old id, and prints `set aside …` after
//! `node`), materialises the RegistryApi fold, and writes its own presence and
//! the binding of its endpoint key (plan Step 4.2; a boot after the key was
//! replaced revokes the old key's and prints `revoked …` after `node`) —
//! the node serves itself from its own disk BEFORE any client connects (the
//! s-boot trace). The registry then seeds the served store (the home share is
//! an ORDINARY share, GDL-038), the iroh peer endpoint binds with the node's
//! directory identity and its `endpoint.key`, and accepts inbound peer links
//! (prints `peer <endpoint-id> <ip:port>` — the dial target for a `--peer`
//! flag on another node, the same at every start), and each `--peer` target
//! is dialed and the home share converged. Then it serves the app-data
//! carrier as before.
//!
//! The endpoint has a door (plan Step 4.2b): it admits an endpoint key bound
//! by a record the node holds, or one a `--peer` entry names on first
//! contact, and refuses every other, reporting `peer refused: endpoint <id>:
//! <reason>` on stderr. `--peer ID` names a key to admit and dial nothing;
//! `--peer ID@IP:PORT` names one and dials it.
//!
//! Each `--app FILE.glade` is LOADED as data and REGISTERED (GDL-037): its
//! declarations append as ordinary records, its ACL seeds compile to grant
//! records — under this node's chain, diffed against the fold (idempotent).
//! An app is declared by one file. Every file is loaded before the instance
//! is opened, so a file that cannot be read or fails to parse, or two files
//! naming one app, stop the node before it writes anything; a file that
//! parses with warnings prints each to stderr as `<FILE>: warning: line N:
//! ...` and boots.
//!
//! A start that fails, a refused `--app` file included, prints its message
//! to stderr as the author reads it, `<FILE>: line N: ...` for a file that
//! breaks a rule, and exits 1.
//!
//! Either form binds 127.0.0.1:<port> (0 = OS-assigned) and prints
//! `listening <port>` so a parent process can read the actual port.
//!
//! **Two composition roots** (plan Step 3.2). The environment variable
//! `GLADE_NODE_ASSEMBLED` chooses which one starts the node:
//!
//! - unset: the hand-written straight line, `run` below, which the demo and
//!   grazel use;
//! - `1`: the assembled root, `run_assembled`, which starts the sdax plan
//!   `glade_node::lifecycle::node_plan` (plan Step 3.3). The plan acquires the
//!   instance, the store, the peer endpoint and the listener, resolves its
//!   bindings from the Shaku module `glade_node::assembly::NodeAssembly`, and
//!   composes the same `Server` calls. It prints the same lines, and before
//!   them one stderr line naming itself (`assembly::ASSEMBLED_ROOT_LINE`).
//!   SIGTERM or SIGINT stops it: the plan stops admitting work, cancels its
//!   tasks and any start-up still in flight, and releases what it acquired, in
//!   reverse. A clean stop exits 0; anything else prints why on stderr and
//!   exits 1;
//! - any other value, the empty string included: the start is refused before
//!   anything is read or written, with a message naming the variable, and
//!   the node exits 1.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use glade_node::assembly::{Settings, ASSEMBLED_ROOT_LINE};
use glade_node::iroh_carrier::{PeerEndpoint, PeerEntry};
use glade_node::lifecycle::{conclude, node_plan, Console, NodeStart, StdConsole};
use glade_node::registry::{RegistryApi, StoreApi, HOME};
use glade_node::server::Server;
use glade_node::sysdir::{boot, now_ms, Profile};
use glade_node::transport::Door;
use sdax_tokio::{PlanStart, TokioRuntime};
use tokio::net::TcpListener;

/// The variable that chooses the composition root.
const ASSEMBLED: &str = "GLADE_NODE_ASSEMBLED";

/// Which composition root starts the node: `GLADE_NODE_ASSEMBLED` unset is
/// the hand-written one, `1` the assembled one, and anything else is refused.
fn assembled(value: Option<OsString>) -> std::io::Result<bool> {
    match value {
        None => Ok(false),
        Some(value) if value == "1" => Ok(true),
        Some(value) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{ASSEMBLED}={:?}: expected 1, the assembled composition root, or unset, the hand-written one",
                value.to_string_lossy()
            ),
        )),
    }
}

/// Start the node from the composition root the environment chooses.
async fn start() -> std::io::Result<ExitCode> {
    if assembled(std::env::var_os(ASSEMBLED))? {
        return run_assembled().await;
    }
    run().await.map(|()| ExitCode::SUCCESS)
}

/// Runs the node, and prints a failure with `Display`, its message as written
/// (SUR-P3-11): returning the error from `main` would print its `Debug` form,
/// `Error: Custom { kind: InvalidData, error: "..." }`, with the message's
/// quotes escaped. A failure exits 1 (`ExitCode::FAILURE`), as it did.
#[tokio::main]
async fn main() -> ExitCode {
    match start().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> std::io::Result<()> {
    let mut profile: Option<Profile> = None;
    let mut name: Option<String> = None;
    let mut operator: Option<String> = None;
    let mut apps: Vec<String> = Vec::new();
    let mut peers: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = args.next().and_then(|s| Profile::parse(&s)),
            "--name" => name = args.next(),
            "--operator" => operator = args.next(),
            "--app" => apps.extend(args.next()),
            "--peer" => peers.extend(args.next()),
            _ => positional.push(a),
        }
    }
    let port: u16 = positional.first().and_then(|s| s.parse().ok()).unwrap_or(0);

    // ---- sysdir boot is OPT-IN (GDL-036) ------------------------------------
    // Only an explicit --profile/--name boots the system-data instance; the
    // legacy positional form keeps its pre-seam contract exactly.
    let booted = if profile.is_some() || name.is_some() {
        // Every `--app` file is loaded, and two naming one app are refused,
        // before `boot` opens the instance (L1-14): a refused start writes
        // nothing.
        let decls = glade_node::appdecl::load_all(&apps)?;
        let mut node = boot(profile.unwrap_or(Profile::Local), name.as_deref(), operator.as_deref())?;
        println!("instance {}", node.dir.display());
        println!("node {}", node.node_id);
        if let Some(aside) = &node.set_aside {
            println!("{aside}");
        }
        if let Some(revoked) = node.rebound.line() {
            println!("{revoked}");
        }
        if node.rejected > 0 {
            println!("quarantined {} record(s) at load", node.rejected);
        }
        let serves_home = node.registry.who_serves(HOME, now_ms()).is_some();
        println!("registry ready (home served: {serves_home})");
        // ---- app registration (GDL-037): <app>.glade loaded as data --------
        // Ordinary attributed appends under this node's chain, diffed against
        // the fold (idempotent), then persisted like any other record write.
        let registrant = node.node_id.clone();
        let mut workspaces: Vec<(String, String)> = Vec::new();
        for (path, decl) in apps.iter().zip(decls) {
            // The non-fatal channel (R10(a)): the file loaded, so boot goes on;
            // each warning goes to stderr, path-prefixed as `load`'s errors are.
            for line in decl.warning_lines(path) {
                eprintln!("{line}");
            }
            let reg = glade_node::appdecl::register(&decl, &mut node.registry, &registrant)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:?}")))?;
            node.store.save(&node.registry.snapshot())?;
            println!("app {} registered (+{} record(s), {} unchanged)", decl.app, reg.appended, reg.unchanged);
            workspaces.extend(decl.workspaces.iter().map(|w| (w.share.clone(), w.name.clone())));
        }
        Some((node, workspaces))
    } else {
        None
    };

    // ---- serve app data (unchanged carrier) --------------------------------
    // App-data store dir: the second positional; else, when booted, a `store/`
    // under the instance's class-4 cache (rebuildable, never load-bearing for
    // system data); else the legacy temp-dir default.
    let dir = positional.get(1).cloned().unwrap_or_else(|| match &booted {
        Some((node, _)) => node.dir.join("cache").join("store").to_string_lossy().into_owned(),
        None => std::env::temp_dir().join("glade-node-bin").to_string_lossy().into_owned(),
    });

    let server = Server::open(&dir)?;

    // ---- peer fabric (booted forms only; the legacy form never binds it) ----
    // Adopt the boot instance (seeds the served store; the boot registry stays
    // the chain authority for this node's own directory writes — claims.rs),
    // bind iroh with the DIRECTORY identity, run the accept loop, converge
    // with each `--peer` target, then start SERVING the declared workspaces:
    // mint WorkspaceEntry + ServeClaim and renew while serving (audit F1).
    if let Some((node, workspaces)) = booted {
        let (identity, key) = (node.identity()?, node.endpoint_key());
        server.adopt_boot(node).await?;
        let entries: Vec<Option<PeerEntry>> = peers.iter().map(|p| PeerEntry::parse(p)).collect();
        let configured = entries.iter().flatten().map(PeerEntry::key);
        let door = Arc::new(Door::new(configured, |line: &str| eprintln!("{line}")));
        let endpoint = PeerEndpoint::bind_door(identity, key, door).await?;
        let addr = server.enable_mesh(endpoint).await?;
        println!("peer {} {}", addr.endpoint_id, addr.socket);
        for (p, entry) in peers.iter().zip(&entries) {
            match entry {
                Some(PeerEntry::Dial(target)) => match server.connect_peer(target).await {
                    Ok(id) => println!("peer-connected {id}"),
                    Err(e) => eprintln!("peer {p}: {e}"),
                },
                Some(PeerEntry::Known(_)) => {}
                None => eprintln!("peer {p}: expected <endpoint-id> or <endpoint-id>@<ip:port>"),
            }
        }
        for (share, name) in &workspaces {
            server.serve_workspace(share, name).await?;
            println!("workspace {share} serving");
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let actual = listener.local_addr()?.port();
    println!("listening {actual}");
    std::io::stdout().flush().ok();
    server.run(listener).await
}

/// The assembled composition root (plan Steps 3.2 and 3.3): `run`'s start,
/// step for step, as the sdax plan `glade_node::lifecycle::node_plan`, which
/// owns every acquisition and every task. The root parses the arguments and
/// loads every `--app` file, then waits on the plan where `run` waits on
/// `server.run`. A stop signal asks the plan to shut down; the report decides
/// the exit status.
async fn run_assembled() -> std::io::Result<ExitCode> {
    eprintln!("{ASSEMBLED_ROOT_LINE}");
    let settings = Settings::from_args(std::env::args().skip(1));
    // Every `--app` file is loaded, and two naming one app are refused,
    // before the plan boots the instance (L1-14): a refused start writes
    // nothing.
    let decls = if settings.booted() {
        glade_node::appdecl::load_all(&settings.apps)?
    } else {
        Vec::new()
    };
    let console: Arc<dyn Console> = Arc::new(StdConsole);
    let start = NodeStart::from_settings(settings, decls, console.clone());
    let mut stop = stop_signal::StopSignal::install()?;
    let runtime = Arc::new(TokioRuntime::new(tokio::runtime::Handle::current()));
    let mut running = node_plan().start(runtime, start);
    let run = running.handle();
    let report = loop {
        tokio::select! {
            report = &mut running => break report,
            // A normal end, from wherever the run is: start-up bodies still in
            // flight are cancelled, and the release graph runs within the
            // plan's shutdown budget. sdax never interrupts a cleanup that has
            // begun (INV-7), so a later signal changes nothing.
            () = stop.received() => run.shutdown(),
        }
    };
    if conclude(&report, &*console) {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

// The stop signal the assembled root waits for: SIGTERM or SIGINT on Unix,
// Ctrl-C elsewhere. Each platform's branch is one braced module, so the
// condition encloses the whole section.
#[cfg(unix)]
mod stop_signal {
    use tokio::signal::unix::{signal, Signal, SignalKind};

    pub struct StopSignal {
        term: Signal,
        interrupt: Signal,
    }

    impl StopSignal {
        /// Install the handlers: from here on the signals no longer end the
        /// process themselves.
        pub fn install() -> std::io::Result<StopSignal> {
            Ok(StopSignal {
                term: signal(SignalKind::terminate())?,
                interrupt: signal(SignalKind::interrupt())?,
            })
        }

        /// Resolves at the next SIGTERM or SIGINT.
        pub async fn received(&mut self) {
            tokio::select! {
                _ = self.term.recv() => {}
                _ = self.interrupt.recv() => {}
            }
        }
    }
}

#[cfg(not(unix))]
mod stop_signal {
    pub struct StopSignal;

    impl StopSignal {
        pub fn install() -> std::io::Result<StopSignal> {
            Ok(StopSignal)
        }

        /// Resolves at the next Ctrl-C; never, if it cannot be listened for.
        pub async fn received(&mut self) {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}
