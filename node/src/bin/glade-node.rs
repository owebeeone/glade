//! `glade-node PORT STORE_DIR`, or `glade-node --profile ... [PORT] [STORE_DIR]`
//! — run a glade node (GLP-0005 + GDL-036).
//!
//! Two invocation forms:
//!
//! **Legacy serve form** (neither `--profile` nor `--name`): `glade-node
//! <port> <store_dir>` — the pre-seam contract, but for one change: the store
//! directory is REQUIRED (the owner's ruling of 2026-09-26). Serve the
//! app-data carrier from `store_dir`, NO sysdir boot, NO `~/.glade` access.
//! Started without `store_dir`, the node prints the usage line to stderr and
//! exits 1, having written nothing; it once stored in a temp dir that every
//! such node shared, unlocked. The grip-share integration suite spawns this
//! form (concurrently — a global singleton lock here would collide, and tests
//! must never write $HOME).
//!
//! **Booted profile form** (opt-in): `glade-node --profile local|peer|server
//! [--name NAME] [--operator OP] [--app FILE.glade]... [--config PATH]
//! [--peer ID[@IP:PORT|@RELAY-URL]]... [--enforce-client-grants] [PORT]
//! [STORE_DIR]` —
//! reads every `--app` file, then the network (plan Step 4.5: the `--config`
//! file, an absolute path no one else may read, and the `--peer` flags; a
//! file that cannot be read, a bad line or a bad flag refuses the start
//! before anything is written), then boots the system-data instance (GDL-036): acquires
//! `~/.glade/sys/<name>/` (the profile picks the default name; `--name`
//! overrides, with a name that matches `[A-Za-z0-9._-]{1,63}` and does not
//! end in `.`, or the start is refused before anything is written, as
//! `recovery` and `endpoint-id` refuse it (F9);
//! `GLADE_HOME` overrides `$HOME/.glade`, and each composition root
//! reads the two once, as it starts, and passes the root down), runs the load-validation
//! ladder (the first boot after plan Step 4.1b also sets aside, once, the
//! unsigned records written before it, and prints `set aside …` after
//! `node`), materialises the RegistryApi fold, and writes its own presence and
//! the binding of its endpoint key (plan Step 4.2; a boot after the key was
//! replaced revokes the old key's and prints `revoked …` after `node`) —
//! the node serves itself from its own disk BEFORE any client connects (the
//! s-boot trace). The served store opens, setting aside any `home` journal
//! that does not verify (plan Step 4.1b; one more `set aside …` line). The
//! registry then seeds it (the home share is an ORDINARY share, GDL-038), and
//! the node's `home` claim is renewed, as it is every 100 s while the node runs,
//! before the node prints `registry ready (home served: …)`. The iroh peer
//! endpoint binds with the node's directory identity and its `endpoint.key`,
//! where the network says (with no file, `127.0.0.1:0` alone, no relay, no
//! portmapper, on every profile), and accepts inbound peer links (prints
//! `peer <tag> <ip:port>`: the first 10 hex digits of its endpoint id, the
//! same at every start, and its address as bound), and each peer with an
//! address is dialed and the home share converged. Then it serves the
//! app-data carrier as before.
//!
//! The endpoint has a door (plan Step 4.2b): it admits an endpoint key bound
//! by a record the node holds, or one a peer entry names on first contact,
//! and refuses every other, reporting `peer refused: endpoint <tag>:
//! <reason>` on stderr. An entry `ID` names a key to admit and dial nothing;
//! `ID@IP:PORT`, or `ID@RELAY-URL` with `relay n0`, names one and dials it.
//! A failed dial says `peer <tag>@<address>: <error>` on stderr. No line
//! holds an endpoint id: `glade-node endpoint-id --name NAME` prints one, and
//! starts no node (plan Step 4.5): it reads the instance's `endpoint.key`, or
//! mints one first, and prints its id alone.
//!
//! A linked node notes on stdout what iroh does (plan Step 4.5): `link <node>
//! via relay <url>|direct <ip:port>, rtt <n> ms` at HELLO and whenever iroh
//! selects another path, `link <node> closed`, and `home round with node
//! <node>: <n> record(s) in <ms> ms` when its pull from that peer ends. With
//! `relay n0` it notes `relay <url>` when a home relay connects, and `relay
//! <url> not connected: <error>` when that fails or drops. A node with no
//! link and no relay, as the desk runs, notes nothing.
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
//! **The recovery key** (plan Step 4.1c; `GladeNodeSigning.md` D10 (a)). A
//! booted node that has committed no recovery key says, on stderr after its
//! boot lines, exactly what to run, and starts: `no recovery key is committed
//! for this node: stop it, then run GLADE_HOME=<root> <program> recovery
//! --name <name> --out <an absolute path outside GLADE_HOME>`, each path
//! quoted as a POSIX shell reads it back, and the program named without the
//! verbatim prefix `\\?\` it has on Windows (F11). That is the
//! third form, a one-shot command on the stopped instance: `glade-node
//! recovery --name NAME --out PATH` commits a recovery key in the node's
//! chain, writes its secret to the new file PATH (absolute, outside
//! `GLADE_HOME`, 0600) and nowhere else, prints `node <id>` and `recovery key
//! <hex> committed; ...`, and exits. It starts no node, so
//! `GLADE_NODE_ASSEMBLED` does not apply to it. A new node can take
//! `--recovery-out PATH` in the booted form instead: its first boot commits the
//! key in the save that writes its presence, and prints that line after
//! `node`; any later boot given it is refused. A `local.json` that fails its
//! check (plan Step 4.1c) is discarded to its fail-closed defaults, with a
//! line on stderr, and the node starts.
//!
//! The program reads its arguments, `GLADE_HOME` and `HOME`, and its own path
//! once, at its entry point, and passes them down: nothing below reads the
//! environment (the owner's rule of no process globals, glade's `AGENTS.md`).
//! The entry point also hands each root the node's leases (F1, the owner's
//! ruling of 2026-09-27): each claim lives five minutes and is renewed every
//! 100 s. No flag changes them.
//!
//! Either form binds 127.0.0.1:<port> (0 = OS-assigned) and prints
//! `listening <port>` so a parent process can read the actual port.
//!
//! The grant check (plan Step 4.3): a peer reads a share this node serves only
//! with a grant from this node's fold, always. A client session is checked
//! too only with `--enforce-client-grants`, which is off by default; then the
//! node prints `client grants enforced: …` before it serves.
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
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use glade_node::assembly::{Settings, ASSEMBLED_ROOT_LINE};
use glade_node::claims::Leases;
use glade_node::endpoint_id;
use glade_node::grants::{CLIENT_GRANTS_ENFORCED, GRANTS_UNAVAILABLE};
use glade_node::iroh_carrier::{IrohCarrier, Lent, FIRST_WORD};
use glade_node::lifecycle::{conclude, node_plan, Console, NodeStart, StdConsole};
use glade_node::mesh::PeerPort;
use glade_node::netconf;
use glade_node::recovery;
use glade_node::registry::{RegistryApi, StoreApi, HOME};
use glade_node::server::Server;
use glade_node::sysdir::{boot, instance_root, Profile};
use glade_node::transport::Door;
use sdax_tokio::{PlanStart, TokioRuntime};
use tokio::net::TcpListener;

/// The variable that chooses the composition root.
const ASSEMBLED: &str = "GLADE_NODE_ASSEMBLED";

/// The line a start the command line cannot run is refused with.
const USAGE: &str = "usage: glade-node <port> <store_dir> (the legacy form requires its \
    store directory), or glade-node --profile local|peer|server [--name NAME] \
    [--operator OP] [--app FILE.glade]... [--config PATH] \
    [--peer ID[@IP:PORT|@RELAY-URL]]... [--enforce-client-grants] [--recovery-out PATH] \
    [port] [store_dir], or glade-node recovery --name NAME --out PATH, or \
    glade-node endpoint-id --name NAME";

/// The refusal of a legacy start with no store directory: the usage line on
/// stderr, and exit 1, as every refused start.
fn usage() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, USAGE)
}

/// The instance root: `GLADE_HOME`, else `$HOME/.glade`. Each composition
/// root, and the recovery command, reads the two variables here, once, as it
/// starts, and passes the root down; nothing below it reads them.
fn instance_root_from_env() -> PathBuf {
    let glade_home = std::env::var("GLADE_HOME").ok();
    let home = std::env::var("HOME").ok();
    instance_root(glade_home, home)
}

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

/// The running program's path, its links resolved, read once here and passed
/// down: the recovery warning names it (plan Step 4.1c). Read below the entry
/// point, it would name whatever program the library runs in.
fn program_path() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    Some(std::fs::canonicalize(&program).unwrap_or(program))
}

/// Run the recovery command, or start the node from the composition root the
/// environment chooses. The process's arguments are read here, once, and
/// handed to whichever runs, with the node's leases (F1): the defaults, a
/// five-minute lease renewed every 100 s, which no flag changes.
async fn start() -> std::io::Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "recovery") {
        let root = instance_root_from_env();
        for line in recovery::command(&root, args.into_iter().skip(1))? {
            println!("{line}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    if args.first().is_some_and(|arg| arg == "endpoint-id") {
        let root = instance_root_from_env();
        println!("{}", endpoint_id::command(&root, args.into_iter().skip(1))?);
        return Ok(ExitCode::SUCCESS);
    }
    let program = program_path();
    let leases = Leases::default();
    if assembled(std::env::var_os(ASSEMBLED))? {
        return run_assembled(args, program, leases).await;
    }
    run(args, program, leases).await.map(|()| ExitCode::SUCCESS)
}

/// A status line of the mesh's (plan Step 4.5), on stdout. The notes come
/// after `listening`, which is where a parent may stop reading, so a line
/// stdout cannot take is dropped: it never stops the node, as `println!`'s
/// panic would.
fn noted(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
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

async fn run(args: Vec<String>, program: Option<PathBuf>, leases: Leases) -> std::io::Result<()> {
    let mut profile: Option<Profile> = None;
    let mut name: Option<String> = None;
    let mut operator: Option<String> = None;
    let mut apps: Vec<String> = Vec::new();
    let mut config: Option<String> = None;
    let mut peers: Vec<String> = Vec::new();
    let mut enforce_client_grants = false;
    let mut recovery_out: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = args.next().and_then(|s| Profile::parse(&s)),
            "--name" => name = args.next(),
            "--operator" => operator = args.next(),
            "--app" => apps.extend(args.next()),
            "--config" => config = args.next(),
            "--peer" => peers.extend(args.next()),
            "--enforce-client-grants" => enforce_client_grants = true,
            "--recovery-out" => recovery_out = args.next(),
            _ => positional.push(a),
        }
    }
    let port: u16 = positional.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let root = instance_root_from_env();

    // ---- sysdir boot is OPT-IN (GDL-036) ------------------------------------
    // Only an explicit --profile/--name boots the system-data instance; the
    // legacy positional form keeps its pre-seam contract exactly.
    let booted = if profile.is_some() || name.is_some() {
        // Every `--app` file is loaded, and two naming one app are refused,
        // before `boot` opens the instance (L1-14): a refused start writes
        // nothing.
        let decls = glade_node::appdecl::load_all(&apps)?;
        // The network (plan Step 4.5): the file, then the flags, each checked
        // before the instance boots.
        let network = netconf::load(config.as_deref(), &peers)?;
        let profile = profile.unwrap_or(Profile::Local);
        let recovery_out = recovery_out.as_deref().map(Path::new);
        let (name, operator, lease_ms) = (name.as_deref(), operator.as_deref(), leases.lease_ms);
        let mut node = boot(&root, profile, name, operator, recovery_out, lease_ms)?;
        println!("instance {}", node.dir.display());
        println!("node {}", node.node_id);
        if let Some(committed) = &node.recovery {
            println!("{committed}");
        }
        if let Some(aside) = &node.set_aside {
            println!("{aside}");
        }
        if let Some(revoked) = node.rebound.line() {
            println!("{revoked}");
        }
        if node.rejected > 0 {
            println!("quarantined {} record(s) at load", node.rejected);
        }
        if node.registry.policy_quarantined() {
            println!("{GRANTS_UNAVAILABLE}");
        }
        // Plan Step 4.1c: a local.json that failed its check, and a node with
        // no recovery key committed, are said on stderr; the start goes on.
        if let Some(discarded) = &node.overlay.discarded {
            eprintln!("{discarded}");
        }
        if let Some(warning) = recovery::warning(&node, program.as_deref()) {
            eprintln!("{warning}");
        }
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
        Some((node, workspaces, network))
    } else {
        None
    };

    // ---- serve app data (unchanged carrier) --------------------------------
    // App-data store dir: the second positional; else, when booted, a `store/`
    // under the instance's class-4 cache (rebuildable, never load-bearing for
    // system data). The legacy form requires it: without it the start is
    // refused here, having read and written nothing.
    let dir = match (positional.get(1), &booted) {
        (Some(dir), _) => dir.clone(),
        (None, Some((node, ..))) => {
            let store = node.dir.join("cache").join("store");
            store.to_string_lossy().into_owned()
        }
        (None, None) => return Err(usage()),
    };

    let server = Server::open(&dir)?;
    if let Some(aside) = server.set_aside().await {
        println!("{aside}");
    }
    if enforce_client_grants {
        server.enforce_client_grants();
        println!("{CLIENT_GRANTS_ENFORCED}");
    }

    // ---- peer fabric (booted forms only; the legacy form never binds it) ----
    // Adopt the boot instance (seeds the served store; the boot registry stays
    // the chain authority for this node's own directory writes — claims.rs —
    // and the `home` claim is renewed from here on), bind the peer adapter,
    // lent the endpoint key and the door (plan Step 4.5b), run the mesh on it
    // with the DIRECTORY identity, converge with each `--peer` target, then
    // start SERVING the declared workspaces: mint WorkspaceEntry + ServeClaim
    // and renew while serving (audit F1).
    if let Some((node, workspaces, network)) = booted {
        let (identity, key) = (node.identity()?, node.endpoint_key());
        let (lease_ms, renew_ms) = (leases.lease_ms, leases.renew_ms);
        server.adopt_boot_tuned(node, lease_ms, renew_ms).await?;
        let serves_home = server.serves(HOME).await.is_some();
        println!("registry ready (home served: {serves_home})");
        let configured = network.peers.iter().map(|entry| entry.key);
        let door = Door::new(configured, |line: &str| eprintln!("{line}"));
        let door = Arc::new(door.with_status(noted));
        let (relays, first_word) = (network.relays, FIRST_WORD);
        let lent = Lent {
            key,
            door: Some(door.clone()),
            relays,
            first_word,
        };
        let carrier = IrohCarrier::new(Some(lent));
        let line = carrier.bind_network(&network).await?;
        let peer = PeerPort::iroh(&carrier, identity, &key, Some(door));
        server.enable_mesh(peer).await?;
        println!("{line}");
        for entry in network.peers.iter().filter(|entry| !entry.via.is_empty()) {
            match server.connect_peer(entry.clone()).await {
                Ok(id) => println!("peer-connected {id}"),
                Err(e) => eprintln!("peer {entry}: {e}"),
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
/// owns every acquisition and every task. The root parses the arguments
/// `start` read, puts the instance root, the program's path and the leases
/// into the settings, and loads every `--app` file, then waits on the plan
/// where `run` waits on `server.run`. A stop signal
/// asks the plan to shut down; the report decides the exit status.
async fn run_assembled(
    args: Vec<String>,
    program: Option<PathBuf>,
    leases: Leases,
) -> std::io::Result<ExitCode> {
    eprintln!("{ASSEMBLED_ROOT_LINE}");
    let mut settings = Settings {
        instance_root: Some(instance_root_from_env()),
        program,
        leases,
        ..Settings::from_args(args)
    };
    // The legacy form requires its store directory: refused, as `run`
    // refuses it, before the plan starts.
    if !settings.booted() && settings.store_dir().is_none() {
        return Err(usage());
    }
    // Every `--app` file is loaded, and two naming one app are refused, then
    // the network (plan Step 4.5), before the plan boots the instance
    // (L1-14): a refused start writes nothing.
    let decls = if settings.booted() {
        let decls = glade_node::appdecl::load_all(&settings.apps)?;
        settings.network = netconf::load(settings.config.as_deref(), &settings.peers)?;
        decls
    } else {
        Vec::new()
    };
    let console: Arc<dyn Console> = Arc::new(StdConsole);
    let start = NodeStart::from_settings(settings, decls, console.clone())?;
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
