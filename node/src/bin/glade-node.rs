//! `glade-node <port> [store_dir]` — run the localhost glade node (GLP-0005).
//! Binds 127.0.0.1:<port> (0 = OS-assigned) and prints `listening <port>` so a
//! parent process can read the actual port, then serves until killed.

use std::io::Write;

use glade_node::server::Server;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let dir = args
        .next()
        .unwrap_or_else(|| std::env::temp_dir().join("glade-node-bin").to_string_lossy().into_owned());

    let server = Server::open(&dir)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let actual = listener.local_addr()?.port();
    println!("listening {actual}");
    std::io::stdout().flush().ok();
    server.run(listener).await
}
