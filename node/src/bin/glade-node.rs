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
//! [--name NAME] [--operator OP] [--peer ID@IP:PORT]... [PORT] [STORE_DIR]` —
//! FIRST boots the system-data instance (GDL-036): acquires
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
//! Either form binds 127.0.0.1:<port> (0 = OS-assigned) and prints
//! `listening <port>` so a parent process can read the actual port.

use std::io::Write;

use glade_node::iroh_carrier::{PeerAddr, PeerEndpoint};
use glade_node::registry::{RegistryApi, HOME};
use glade_node::server::Server;
use glade_node::sysdir::{boot, now_ms, Profile};
use tokio::net::TcpListener;

/// Parse a `--peer` target: `<endpoint-id-hex>@<ip:port>` (the two values a
/// peer prints as `peer <id> <addr>`).
fn parse_peer(s: &str) -> Option<PeerAddr> {
    let (id, sock) = s.split_once('@')?;
    Some(PeerAddr { endpoint_id: id.parse().ok()?, socket: sock.parse().ok()? })
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut profile: Option<Profile> = None;
    let mut name: Option<String> = None;
    let mut operator: Option<String> = None;
    let mut peers: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = args.next().and_then(|s| Profile::parse(&s)),
            "--name" => name = args.next(),
            "--operator" => operator = args.next(),
            "--peer" => peers.extend(args.next()),
            _ => positional.push(a),
        }
    }
    let port: u16 = positional.first().and_then(|s| s.parse().ok()).unwrap_or(0);

    // ---- sysdir boot is OPT-IN (GDL-036) ------------------------------------
    // Only an explicit --profile/--name boots the system-data instance; the
    // legacy positional form keeps its pre-seam contract exactly.
    let booted = if profile.is_some() || name.is_some() {
        let node = boot(profile.unwrap_or(Profile::Local), name.as_deref(), operator.as_deref())?;
        println!("instance {}", node.dir.display());
        println!("node {}", node.node_id);
        if node.rejected > 0 {
            println!("quarantined {} record(s) at load", node.rejected);
        }
        let serves_home = node.registry.who_serves(HOME, now_ms()).is_some();
        println!("registry ready (home served: {serves_home})");
        Some(node)
    } else {
        None
    };

    // ---- serve app data (unchanged carrier) --------------------------------
    // App-data store dir: the second positional; else, when booted, a `store/`
    // under the instance's class-4 cache (rebuildable, never load-bearing for
    // system data); else the legacy temp-dir default.
    let dir = positional.get(1).cloned().unwrap_or_else(|| match &booted {
        Some(node) => node.dir.join("cache").join("store").to_string_lossy().into_owned(),
        None => std::env::temp_dir().join("glade-node-bin").to_string_lossy().into_owned(),
    });

    let server = Server::open(&dir)?;

    // ---- peer fabric (booted forms only; the legacy form never binds it) ----
    // Seed the served store from the boot registry (dir.* becomes an ordinary
    // share on the ordinary rails), bind iroh with the DIRECTORY identity, run
    // the accept loop, and converge with each `--peer` target.
    if let Some(node) = &booted {
        server.seed_registry(&node.registry.snapshot()).await;
        let endpoint = PeerEndpoint::bind_with(node.identity()?).await?;
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
    }

    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let actual = listener.local_addr()?.port();
    println!("listening {actual}");
    std::io::stdout().flush().ok();
    server.run(listener).await
}
