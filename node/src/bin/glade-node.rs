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
//! [--name NAME] [--operator OP] [--app FILE.glade]... [--peer ID@IP:PORT]...
//! [PORT] [STORE_DIR]` —
//! reads every `--app` file, then boots the system-data instance (GDL-036): acquires
//! `~/.glade/sys/<name>/` (the profile picks the default name; `--name`
//! overrides; `GLADE_HOME` overrides `$HOME/.glade`), runs the load-validation
//! ladder, materialises the RegistryApi fold, and writes its own presence —
//! the node serves itself from its own disk BEFORE any client connects (the
//! s-boot trace). The registry then seeds the served store (the home share is
//! an ORDINARY share, GDL-038), the iroh peer endpoint binds with the node's
//! directory identity and accepts inbound peer links (prints
//! `peer <endpoint-id> <ip:port>` — the dial target for a `--peer` flag on
//! another node), and each `--peer` target is dialed and the home share
//! converged. Then it serves the app-data carrier as before.
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
//! - `1`: the assembled root, `run_assembled`, which resolves its bindings
//!   from the Shaku module `glade_node::assembly::NodeAssembly` and composes
//!   the same `Server` calls. It prints the same lines, and before them one
//!   stderr line naming itself (`assembly::ASSEMBLED_ROOT_LINE`);
//! - any other value, the empty string included: the start is refused before
//!   anything is read or written, with a message naming the variable, and
//!   the node exits 1.

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};

use glade_node::assembly::{
    CommandLine, Config, Directory, InstanceSlot, NodeAssembly, Records, Settings,
    ASSEMBLED_ROOT_LINE,
};
use glade_node::iroh_carrier::{PeerAddr, PeerEndpoint};
use glade_node::registry::{RegistryApi, StoreApi, HOME};
use glade_node::server::Server;
use glade_node::sysdir::{boot, now_ms, Profile};
use shaku::HasComponent;
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
async fn start() -> std::io::Result<()> {
    if assembled(std::env::var_os(ASSEMBLED))? {
        return run_assembled().await;
    }
    run().await
}

/// Parse a `--peer` target: `<endpoint-id-hex>@<ip:port>` (the two values a
/// peer prints as `peer <id> <addr>`).
fn parse_peer(s: &str) -> Option<PeerAddr> {
    let (id, sock) = s.split_once('@')?;
    Some(PeerAddr { endpoint_id: id.parse().ok()?, socket: sock.parse().ok()? })
}

/// Runs the node, and prints a failure with `Display`, its message as written
/// (SUR-P3-11): returning the error from `main` would print its `Debug` form,
/// `Error: Custom { kind: InvalidData, error: "..." }`, with the message's
/// quotes escaped. A failure exits 1 (`ExitCode::FAILURE`), as it did.
#[tokio::main]
async fn main() -> ExitCode {
    match start().await {
        Ok(()) => ExitCode::SUCCESS,
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
        let identity = node.identity()?;
        server.adopt_boot(node).await?;
        let endpoint = PeerEndpoint::bind_with(identity).await?;
        let addr = server.enable_mesh(endpoint).await?;
        println!("peer {} {}", addr.endpoint_id, addr.socket);
        for p in &peers {
            match parse_peer(p) {
                Some(target) => match server.connect_peer(&target).await {
                    Ok(id) => println!("peer-connected {id}"),
                    Err(e) => eprintln!("peer {p}: {e}"),
                },
                None => eprintln!("peer {p}: expected <endpoint-id>@<ip:port>"),
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

/// The assembled composition root (plan Step 3.2): `run`'s start, step for
/// step, with its bindings resolved from `NodeAssembly`, one node scope.
///
/// Acquisition is I/O, so it stays here and never happens in a constructor.
/// The root boots the instance and lends it to the record host through a
/// slot, then takes it back out, by value, when the `Server` adopts it. It
/// still binds the peer endpoint and the TCP listener itself: their carrier
/// adapters are plan Phase 4's, and until then the `Server` runs both.
async fn run_assembled() -> std::io::Result<()> {
    eprintln!("{ASSEMBLED_ROOT_LINE}");
    let settings = Settings::from_args(std::env::args().skip(1));
    let slot: InstanceSlot = Arc::new(Mutex::new(None));

    // Where the booted instance lives, and the node id its records are
    // attributed to.
    let (decls, booted) = if settings.booted() {
        // Every `--app` file is loaded, and two naming one app are refused,
        // before `boot` opens the instance (L1-14): a refused start writes
        // nothing.
        let decls = glade_node::appdecl::load_all(&settings.apps)?;
        let profile = settings.profile.unwrap_or(Profile::Local);
        let (name, operator) = (settings.name.as_deref(), settings.operator.as_deref());
        let node = boot(profile, name, operator)?;
        println!("instance {}", node.dir.display());
        println!("node {}", node.node_id);
        if node.rejected > 0 {
            println!("quarantined {} record(s) at load", node.rejected);
        }
        let booted = (node.dir.clone(), node.node_id.clone());
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(node);
        (decls, Some(booted))
    } else {
        (Vec::new(), None)
    };

    // The parsed settings and the lent instance are the module's parameters;
    // everything below is resolved from it.
    let module = NodeAssembly::builder()
        .with_component_parameters::<CommandLine>(settings)
        .with_component_parameters::<Records>(slot.clone())
        .build();
    let config: Arc<dyn Config> = module.resolve();
    let settings = config.settings();
    let directory: Arc<dyn Directory> = module.resolve();

    let mut workspaces: Vec<(String, String)> = Vec::new();
    if let Some((_, registrant)) = &booted {
        let serves_home = directory.serves(HOME)?.is_some();
        println!("registry ready (home served: {serves_home})");
        for (path, decl) in settings.apps.iter().zip(decls) {
            // The non-fatal channel (R10(a)), as `run` prints it.
            for line in decl.warning_lines(path) {
                eprintln!("{line}");
            }
            let reg = directory.register(&decl, registrant)?;
            println!(
                "app {} registered (+{} record(s), {} unchanged)",
                decl.app, reg.appended, reg.unchanged
            );
            let declared = decl.workspaces.iter();
            workspaces.extend(declared.map(|w| (w.share.clone(), w.name.clone())));
        }
    }

    let dir = match (settings.store_dir(), &booted) {
        (Some(dir), _) => dir.to_owned(),
        (None, Some((instance, _))) => {
            let store = instance.join("cache").join("store");
            store.to_string_lossy().into_owned()
        }
        (None, None) => {
            let store = std::env::temp_dir().join("glade-node-bin");
            store.to_string_lossy().into_owned()
        }
    };
    let server = Server::open(&dir)?;

    // Adoption: the instance comes back out of the slot by value, and from
    // here on the record host holds nothing (it answers `NotOpen`).
    let adopted = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(node) = adopted {
        let identity = node.identity()?;
        server.adopt_boot(node).await?;
        let endpoint = PeerEndpoint::bind_with(identity).await?;
        let addr = server.enable_mesh(endpoint).await?;
        println!("peer {} {}", addr.endpoint_id, addr.socket);
        for p in &settings.peers {
            match parse_peer(p) {
                Some(target) => match server.connect_peer(&target).await {
                    Ok(id) => println!("peer-connected {id}"),
                    Err(e) => eprintln!("peer {p}: {e}"),
                },
                None => eprintln!("peer {p}: expected <endpoint-id>@<ip:port>"),
            }
        }
        for (share, name) in &workspaces {
            server.serve_workspace(share, name).await?;
            println!("workspace {share} serving");
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", settings.port())).await?;
    let actual = listener.local_addr()?.port();
    println!("listening {actual}");
    std::io::stdout().flush().ok();
    server.run(listener).await
}
