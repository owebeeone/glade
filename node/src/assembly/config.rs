use std::path::PathBuf;

use crate::claims::Leases;
use crate::netconf::Network;
use crate::sysdir::Profile;

// ---- configuration --------------------------------------------------------

/// What `glade-node`'s command line says: its flags, then the positional port
/// and app-data store directory. And the instance root, which the composition
/// root reads from its environment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub profile: Option<Profile>,
    pub name: Option<String>,
    pub operator: Option<String>,
    pub apps: Vec<String>,
    pub peers: Vec<String>,
    /// `--enforce-client-grants` (plan Step 4.3): check client sessions
    /// against the grant fold too. Off by default.
    pub enforce_client_grants: bool,
    pub positional: Vec<String>,
    /// Where the booted form's instance lives, `<root>/sys/<name>`:
    /// `GLADE_HOME`, else `$HOME/.glade` (`sysdir::instance_root`), read once
    /// by the composition root. `None`, as a test's settings leave it, boots
    /// nothing: `NodeStart::from_settings` refuses a booted start without it.
    pub instance_root: Option<PathBuf>,
    /// `--recovery-out PATH` (plan Step 4.1c): where a first boot writes the
    /// secret of the recovery key it commits.
    pub recovery_out: Option<String>,
    /// The running program's path, which the recovery warning names (plan
    /// Step 4.1c), read once by the composition root, as the instance root
    /// is. `None` names `glade-node`.
    pub program: Option<PathBuf>,
    /// How long the node's claims live and how often it renews them (F1):
    /// by default five minutes, renewed every 100 s; a booted start's root
    /// sets them from `lease_ms` when it is given.
    pub leases: Leases,
    /// `--lease-ms <n>` (plan Step 4.6), which the entry point takes out of
    /// the arguments before either root reads them. A booted start takes its
    /// leases from it, or is refused, and prints them after `node`. The
    /// legacy form ignores it, as it ignores `--config` and `--peer`.
    pub lease_ms: Option<String>,
    /// `--config PATH` (plan Step 4.5): the node's network file, which must
    /// be an absolute path. The legacy form ignores it, as it ignores
    /// `--peer`.
    pub config: Option<String>,
    /// The node's network: the `--config` file's, then the `--peer` flags'
    /// (`netconf::load`), which the composition root loads before it builds
    /// `NodeStart`. With no file, `127.0.0.1:0` alone, relays off.
    pub network: Network,
}

impl Settings {
    /// Parse the arguments after the program name exactly as the hand-written
    /// root does: an unknown `--profile` is no profile, a flag given no value
    /// reads as absent, and anything that is not a flag is positional. The
    /// instance root, the program's path and the leases are not arguments,
    /// `--lease-ms` is taken out before this reads them, and the network is
    /// loaded from the file and the flags: the composition root sets them.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Settings {
        let mut settings = Settings::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--profile" => settings.profile = args.next().and_then(|s| Profile::parse(&s)),
                "--name" => settings.name = args.next(),
                "--operator" => settings.operator = args.next(),
                "--app" => settings.apps.extend(args.next()),
                "--recovery-out" => settings.recovery_out = args.next(),
                "--peer" => settings.peers.extend(args.next()),
                "--config" => settings.config = args.next(),
                "--enforce-client-grants" => settings.enforce_client_grants = true,
                _ => settings.positional.push(arg),
            }
        }
        settings
    }

    /// Whether this start boots an instance: `--profile` or `--name` given.
    pub fn booted(&self) -> bool {
        self.profile.is_some() || self.name.is_some()
    }

    /// The port to listen on: the first positional, else 0 (the OS chooses).
    pub fn port(&self) -> u16 {
        self.positional
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// The app-data store directory, when the second positional names one.
    pub fn store_dir(&self) -> Option<&str> {
        self.positional.get(1).map(String::as_str)
    }
}

/// The configuration binding's port. Node-local (plan Step 4.5, the owner's
/// ruling of 2026-09-27): its consumers are all in glade-node.
pub trait ConfigPort: Send + Sync {
    fn settings(&self) -> &Settings;

    /// The node's network: where its endpoint binds, its relays and its
    /// peers.
    fn network(&self) -> &Network {
        &self.settings().network
    }
}
