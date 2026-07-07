//! `glade-node [--profile local|peer|server] [--name NAME] [--operator OP]
//! [PORT] [STORE_DIR]` — run a glade node (GLP-0005 + GDL-036).
//!
//! First it BOOTS the system-data instance: it acquires `~/.glade/sys/<name>/`
//! (a launch profile picks the default name; `--name` overrides; `GLADE_HOME`
//! overrides `$HOME/.glade`), runs the load-validation ladder, materialises the
//! RegistryApi fold, and writes its own presence — the node serves itself from
//! its own disk BEFORE any client connects (the s-boot trace). Then it binds
//! 127.0.0.1:<port> (0 = OS-assigned), prints `listening <port>`, and serves
//! the app-data carrier as before. Positional args stay backward compatible.

use std::io::Write;

use glade_node::registry::{RegistryApi, HOME};
use glade_node::server::Server;
use glade_node::sysdir::{boot, now_ms, Profile};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut profile = Profile::Local;
    let mut name: Option<String> = None;
    let mut operator: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => {
                profile = args
                    .next()
                    .and_then(|s| Profile::parse(&s))
                    .unwrap_or(Profile::Local);
            }
            "--name" => name = args.next(),
            "--operator" => operator = args.next(),
            _ => positional.push(a),
        }
    }
    let port: u16 = positional.first().and_then(|s| s.parse().ok()).unwrap_or(0);

    // ---- boot the system-data instance (the seam, GDL-036) -----------------
    let node = boot(profile, name.as_deref(), operator.as_deref())?;
    println!("instance {}", node.dir.display());
    println!("node {}", node.node_id);
    if node.rejected > 0 {
        println!("quarantined {} record(s) at load", node.rejected);
    }
    let serves_home = node.registry.who_serves(HOME, now_ms()).is_some();
    println!("registry ready (home served: {serves_home})");

    // ---- serve app data (unchanged carrier) --------------------------------
    // App-data store dir: the second positional, else a `store/` under the
    // instance's class-4 cache (rebuildable, never load-bearing for system data).
    let dir = positional
        .get(1)
        .cloned()
        .unwrap_or_else(|| node.dir.join("cache").join("store").to_string_lossy().into_owned());

    let server = Server::open(&dir)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let actual = listener.local_addr()?.port();
    println!("listening {actual}");
    std::io::stdout().flush().ok();
    server.run(listener).await
}
