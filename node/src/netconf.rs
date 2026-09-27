//! The node's network (plan Step 4.5, part 1): where its iroh endpoint binds,
//! whether it has n0's relays, and which peers it admits and dials. A
//! `--config <absolute path>` file sets it, a line each, and the `--peer`
//! flags join their entries after the file's. With no file, every profile's
//! network is as it was: `127.0.0.1:0` alone, relays off, and no peers but the
//! flags'. A file that cannot be read, that others can read or that has one
//! bad line, and a bad flag, refuse the start before anything is written.
//!
//! Carrier-free: no iroh type crosses this module (LBT-004). The two checks
//! that need iroh's own types, a key iroh accepts and a relay in n0's
//! production map, are `iroh_carrier.rs`'s, which also turns a [`Network`]
//! into the endpoint's recipe. A refusal names the file and the line, or the
//! flag's place, and never what the line held, so no message prints an id.
//! The design is `glade/dev-docs/GladeNodeAssembly.md`, "Relay configuration
//! and the first crossing (plan Step 4.5)", sections 2 to 4.

use std::fmt;
use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;

use crate::iroh_carrier::{accepts_endpoint_id, n0_relay};
use crate::transport::{key_of, tag};

/// The relays the endpoint has: none, or n0's production relays (the ruling
/// `relay_posture = community_dev_only`). A relay map of our own waits for a
/// relay of our own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Relays {
    #[default]
    Off,
    N0,
}

/// One way to reach a peer: an address, or one of n0's relays, its URL as the
/// node prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    Ip(SocketAddr),
    Relay(String),
}

/// A peer: its endpoint key, which the door admits on first contact, and
/// where to dial it. No `via` admits it and dials nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerEntry {
    pub key: [u8; 32],
    pub via: Vec<Via>,
}

/// The node's network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    pub relays: Relays,
    /// The sockets the endpoint binds, at most one per address family.
    pub bind: Vec<SocketAddr>,
    /// The file's entries, then the flags', one per key.
    pub peers: Vec<PeerEntry>,
}

impl Default for Network {
    /// No file: `127.0.0.1:0` alone, relays off, no peers.
    fn default() -> Network {
        Network {
            relays: Relays::Off,
            bind: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 0))],
            peers: Vec::new(),
        }
    }
}

impl fmt::Display for Via {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Via::Ip(socket) => write!(f, "{socket}"),
            Via::Relay(url) => f.write_str(url),
        }
    }
}

/// How a line names a peer: by its tag, never its id (section 7), then `@`
/// and each address it is dialed at, if any.
impl fmt::Display for PeerEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&tag(&self.key))?;
        for (n, via) in self.via.iter().enumerate() {
            let before = if n == 0 { "@" } else { "," };
            write!(f, "{before}{via}")?;
        }
        Ok(())
    }
}

// Why a line or a flag is refused. None holds anything the line held.
const KEYWORD: &str = "an unknown keyword; expected relay, bind or peer";
const RELAY: &str = "expected relay off or relay n0";
const SECOND_RELAY: &str = "a second relay line";
const BIND: &str = "expected bind <ip:port>";
const SECOND_IPV4: &str = "a second IPv4 bind";
const SECOND_IPV6: &str = "a second IPv6 bind";
const PEER: &str = "expected <endpoint-id>, <endpoint-id>@<ip:port> or <endpoint-id>@<relay-url>";
const KEY: &str = "the endpoint id is not a key iroh accepts";
const NOT_N0: &str = "the relay URL is not one of n0's production relays";
const NEEDS_N0: &str = "a relay URL needs relay n0";

fn refused(at: &str, why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{at}: {why}"))
}

/// The network a booted start takes: the file at `config`, if one is named,
/// then the `--peer` flags. Each composition root calls it after the app
/// files load and before the instance boots.
pub fn load(config: Option<&str>, flags: &[String]) -> io::Result<Network> {
    let file = config.map(|path| read(Path::new(path))).transpose()?;
    network(file.as_ref().map(borrowed), flags)
}

/// A file's name and its text, borrowed.
fn borrowed(file: &(String, String)) -> (&str, &str) {
    (&file.0, &file.1)
}

/// The file at `path`, as its name and its text: an absolute path, to a file
/// no one else may read.
fn read(path: &Path) -> io::Result<(String, String)> {
    let name = path.display().to_string();
    if !path.is_absolute() {
        return Err(refused(&name, "not an absolute path"));
    }
    platform::check_mode(path)?;
    let text = fs::read_to_string(path);
    let text = text.map_err(|e| io::Error::new(e.kind(), format!("{name}: {e}")))?;
    Ok((name, text))
}

/// The network the file `file`, a name and its text, and the `--peer` flags
/// give.
fn network(file: Option<(&str, &str)>, flags: &[String]) -> io::Result<Network> {
    let mut network = match file {
        Some((name, text)) => parse(name, text)?,
        None => Network::default(),
    };
    for (n, flag) in flags.iter().enumerate() {
        let at = || format!("--peer entry {}", n + 1);
        let entry = entry(flag).map_err(|why| refused(&at(), why))?;
        if needs_n0(&entry, network.relays) {
            return Err(refused(&at(), NEEDS_N0));
        }
        merge(&mut network.peers, entry);
    }
    Ok(network)
}

/// What a file's lines have said so far.
#[derive(Default)]
struct Lines {
    relays: Option<Relays>,
    bind: Vec<SocketAddr>,
    peers: Vec<(usize, PeerEntry)>,
}

impl Lines {
    /// Take in line `n`'s words, a comment and blank space gone.
    fn take(&mut self, n: usize, words: &[&str]) -> Result<(), &'static str> {
        match words {
            [] => Ok(()),
            ["relay", ..] if self.relays.is_some() => Err(SECOND_RELAY),
            ["relay", mode] => self.relay(mode),
            ["relay", ..] => Err(RELAY),
            ["bind", socket] => self.bind(socket),
            ["bind", ..] => Err(BIND),
            ["peer", text] => {
                self.peers.push((n, entry(text)?));
                Ok(())
            }
            ["peer", ..] => Err(PEER),
            _ => Err(KEYWORD),
        }
    }

    fn relay(&mut self, mode: &str) -> Result<(), &'static str> {
        let relays = match mode {
            "off" => Relays::Off,
            "n0" => Relays::N0,
            _ => return Err(RELAY),
        };
        self.relays = Some(relays);
        Ok(())
    }

    fn bind(&mut self, text: &str) -> Result<(), &'static str> {
        let socket: SocketAddr = text.parse().map_err(|_| BIND)?;
        let family = |held: &SocketAddr| held.is_ipv4() == socket.is_ipv4();
        if self.bind.iter().any(family) {
            return Err(match socket {
                SocketAddr::V4(_) => SECOND_IPV4,
                SocketAddr::V6(_) => SECOND_IPV6,
            });
        }
        self.bind.push(socket);
        Ok(())
    }
}

/// The file `name`, whose text is `text`. A `#` starts a comment that runs to
/// the end of its line; blank lines are skipped.
fn parse(name: &str, text: &str) -> io::Result<Network> {
    let at = |n: usize| format!("{name}: line {n}");
    let mut lines = Lines::default();
    for (i, line) in text.lines().enumerate() {
        let kept = line.split('#').next().unwrap_or_default();
        let words: Vec<&str> = kept.split_whitespace().collect();
        let n = i + 1;
        let taken = lines.take(n, &words);
        taken.map_err(|why| refused(&at(n), why))?;
    }
    let bind = if lines.bind.is_empty() {
        Network::default().bind
    } else {
        lines.bind
    };
    let relays = lines.relays.unwrap_or_default();
    let mut network = Network {
        relays,
        bind,
        peers: Vec::new(),
    };
    for (n, entry) in lines.peers {
        if needs_n0(&entry, network.relays) {
            return Err(refused(&at(n), NEEDS_N0));
        }
        merge(&mut network.peers, entry);
    }
    Ok(network)
}

/// An entry's text, `<endpoint-id>`, `<endpoint-id>@<ip:port>` or
/// `<endpoint-id>@<relay-url>`. The id is 64 lower-case hex digits and a key
/// iroh accepts; the relay one of n0's.
fn entry(text: &str) -> Result<PeerEntry, &'static str> {
    let (id, at) = match text.split_once('@') {
        Some((id, at)) => (id, Some(at)),
        None => (text, None),
    };
    let key = key_of(id).ok_or(PEER)?;
    if !accepts_endpoint_id(&key) {
        return Err(KEY);
    }
    let via = match at {
        Some(at) => vec![via(at)?],
        None => Vec::new(),
    };
    Ok(PeerEntry { key, via })
}

/// Where an entry dials: a socket address, else a URL, which must name one of
/// n0's relays.
fn via(text: &str) -> Result<Via, &'static str> {
    if let Ok(socket) = text.parse() {
        return Ok(Via::Ip(socket));
    }
    if !text.contains("://") {
        return Err(PEER);
    }
    n0_relay(text).map(Via::Relay).ok_or(NOT_N0)
}

/// Whether `entry` names a relay the network has not got.
fn needs_n0(entry: &PeerEntry, relays: Relays) -> bool {
    let relayed = entry.via.iter().any(|via| matches!(via, Via::Relay(_)));
    relayed && relays == Relays::Off
}

/// Take `entry` in: a key held already gains its addresses, once each.
fn merge(peers: &mut Vec<PeerEntry>, entry: PeerEntry) {
    let Some(held) = peers.iter_mut().find(|held| held.key == entry.key) else {
        peers.push(entry);
        return;
    };
    for via in entry.via {
        if !held.via.contains(&via) {
            held.via.push(via);
        }
    }
}

// A file's mode is a Unix notion. Each platform's branch is one braced
// module, so the condition encloses the whole section.
#[cfg(unix)]
mod platform {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// Refuse a file that group or others may use in any way, as `node.key`
    /// is refused.
    pub(super) fn check_mode(path: &Path) -> io::Result<()> {
        let name = path.display();
        let held = fs::metadata(path);
        let held = held.map_err(|e| io::Error::new(e.kind(), format!("{name}: {e}")))?;
        let mode = held.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let why = format!("{name} is group/world-accessible (mode {mode:o}) — refusing");
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod platform {
    use std::io;
    use std::path::Path;

    /// Off Unix no mode is checked (F5), as for the key files.
    pub(super) fn check_mode(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{hex, EndpointKey};

    /// A real endpoint id, as `glade-node endpoint-id` prints one.
    fn id(seed: u8) -> String {
        hex(&EndpointKey::from_seed([seed; 32]).endpoint_id)
    }

    /// One of n0's four relays, as the node prints its URL.
    const AP: &str = "https://aps1-1.relay.n0.iroh.link./";

    fn ip(text: &str) -> Via {
        Via::Ip(text.parse().unwrap())
    }

    fn key(seed: u8) -> [u8; 32] {
        EndpointKey::from_seed([seed; 32]).endpoint_id
    }

    /// With no file, every profile's network is as before: `127.0.0.1:0`
    /// alone, relays off, and no peers.
    #[test]
    fn no_file_is_loopback_with_relays_off_and_no_peers() {
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let expected = Network {
            relays: Relays::Off,
            bind: vec![loopback],
            peers: Vec::new(),
        };
        assert_eq!(load(None, &[]).unwrap(), expected);
    }

    /// Each form parses, the relay URL as the node prints it included. Lines
    /// naming one key merge into one entry, dialed at each address they name;
    /// comments and blank lines are skipped; and the `--peer` flags join
    /// after the file's entries, a key the file names gaining its address.
    /// Only parsed: nothing here binds or dials.
    #[test]
    fn the_file_takes_relay_bind_and_peer_lines() {
        let (a, b) = (id(1), id(2));
        let text = format!(
            "# plan Step 4.5, run 2\n\
             relay n0 # n0's relays\n\
             \n\
             bind 10.1.1.236:4545\n\
             bind [fd00::1]:0\n\
             peer {a}\n\
             \tpeer {b}@10.1.1.239:4545\n\
             peer {b}@{AP}\n"
        );
        let flags = [format!("{a}@127.0.0.1:1"), id(3)];
        let network = network(Some(("net.conf", &text)), &flags).unwrap();
        let bind = ["10.1.1.236:4545", "[fd00::1]:0"].map(|s| s.parse().unwrap());
        let peers = vec![
            PeerEntry {
                key: key(1),
                via: vec![ip("127.0.0.1:1")],
            },
            PeerEntry {
                key: key(2),
                via: vec![ip("10.1.1.239:4545"), Via::Relay(AP.into())],
            },
            PeerEntry {
                key: key(3),
                via: Vec::new(),
            },
        ];
        let expected = Network {
            relays: Relays::N0,
            bind: bind.to_vec(),
            peers,
        };
        assert_eq!(network, expected);
    }

    /// Every bad line refuses the whole file, each for its reason, naming the
    /// file and the line, and never what the line held, so no message holds
    /// an id. A bad flag is named by its place.
    #[test]
    fn each_bad_line_is_refused_by_its_number_and_never_echoed() {
        let a = id(1);
        let upper = a.to_uppercase();
        let short = &a[..62];
        // y = 2 is on no point of Ed25519's curve.
        let off_curve = format!("02{}", "00".repeat(31));
        let cases = [
            ("relay n0\nrelays n0\n".to_string(), 2, KEYWORD),
            ("relay n0\nrelay off\n".to_string(), 2, SECOND_RELAY),
            (format!("relay {AP}\n"), 1, RELAY),
            (
                "bind 127.0.0.1:0\nbind 10.0.0.1:0\n".to_string(),
                2,
                SECOND_IPV4,
            ),
            (
                "bind [::1]:0\n# again\nbind [::]:0\n".to_string(),
                3,
                SECOND_IPV6,
            ),
            ("bind 127.0.0.1\n".to_string(), 1, BIND),
            (format!("peer {upper}\n"), 1, PEER),
            (format!("peer {short}\n"), 1, PEER),
            (format!("peer {off_curve}\n"), 1, KEY),
            (format!("peer {a}@{AP}\n"), 1, NEEDS_N0),
            (
                format!("relay n0\npeer {a}@https://relay.example.com./\n"),
                2,
                NOT_N0,
            ),
        ];
        for (text, line, why) in cases {
            let refusal = network(Some(("net.conf", &text)), &[]).unwrap_err();
            let said = format!("net.conf: line {line}: {why}");
            assert_eq!(refusal.to_string(), said, "{text:?}");
            assert_eq!(refusal.kind(), io::ErrorKind::InvalidInput);
        }
        let flags = [id(2), format!("{a}@{AP}")];
        let refusal = network(None, &flags).unwrap_err();
        assert_eq!(refusal.to_string(), format!("--peer entry 2: {NEEDS_N0}"));
    }

    /// Plan Step 4.2b's `--peer` entry, moved here from `iroh_carrier`: an
    /// endpoint id, perhaps with an address. Without one it only configures
    /// the door; with one it is dialed too. Anything else refuses the start,
    /// where it was once skipped with a line.
    #[test]
    fn a_peer_entry_names_a_key_and_perhaps_where_to_dial_it() {
        let a = id(5);
        let flags = [a.clone(), format!("{a}@127.0.0.1:4711")];
        let network = network(None, &flags).unwrap();
        let dialed = PeerEntry {
            key: key(5),
            via: vec![ip("127.0.0.1:4711")],
        };
        assert_eq!(network.peers, [dialed]);
        for junk in ["", "nope", &format!("{a}@"), "@127.0.0.1:1", &a[1..]] {
            let refusal = super::network(None, &[junk.to_string()]).unwrap_err();
            let said = format!("--peer entry 1: {PEER}");
            assert_eq!(refusal.to_string(), said, "{junk:?}");
        }
    }

    /// A line names a peer by its tag, then where it is dialed.
    #[test]
    fn an_entry_is_named_by_its_tag() {
        let entry = PeerEntry {
            key: key(5),
            via: vec![ip("127.0.0.1:4711"), Via::Relay(AP.into())],
        };
        let tag = &id(5)[..10];
        let named = format!("{tag}@127.0.0.1:4711,{AP}");
        assert_eq!(entry.to_string(), named);
    }

    /// The path must be absolute: resolving a relative one would read the
    /// working directory, which only a program's entry point may do. It is
    /// refused before anything is opened.
    #[test]
    fn a_relative_config_path_is_refused() {
        let refusal = load(Some("glade/net.conf"), &[]).unwrap_err();
        let said = "glade/net.conf: not an absolute path";
        assert_eq!(refusal.to_string(), said);
    }

    // File modes are a Unix notion. A braced module, so the condition
    // encloses the whole section.
    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A file group or others can read is refused whatever it holds,
        /// naming the file, as `node.key` is; at 0600 it is taken.
        #[test]
        fn a_config_file_others_can_read_is_refused() {
            let dir = std::env::temp_dir().join("glade-netconf-mode");
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("net.conf");
            fs::write(&path, "relay off\n").unwrap();
            let name = path.display().to_string();
            for mode in [0o640, 0o644] {
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
                let refusal = load(Some(&name), &[]).unwrap_err();
                let said = format!("{name} is group/world-accessible (mode {mode:o}) — refusing");
                assert_eq!(refusal.to_string(), said);
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(load(Some(&name), &[]).unwrap(), Network::default());
            fs::remove_dir_all(&dir).unwrap();
        }
    }
}
