use std::sync::Arc;

use glade_carrier_api::CarrierPort;
use glade_clock_api::ClockPort;
use glade_grant_api::{Denial, GrantPort, Holder};
use shaku::Component;

use crate::appdecl::{AppDecl, Registered};

use super::{ClientCarrier, Clock, Grants, HostError, PeerCarrier, RecordHost, RecordHostPort};

// ---- participants -----------------------------------------------------------

/// The directory facade: reads and writes the directory through the record
/// host, at the injected clock.
pub trait Directory: shaku::Interface {
    /// The clock this directory reads.
    fn clock(&self) -> Arc<dyn ClockPort>;

    /// The record host this directory reads and writes.
    fn host(&self) -> Arc<dyn RecordHostPort>;

    /// Which node serves `share` now, by the injected clock.
    fn serves(&self, share: &str) -> Result<Option<String>, HostError>;

    /// Register an app file's declarations under `origin`.
    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError>;
}

#[derive(Component)]
#[shaku(interface = Directory)]
pub struct DirectoryFacade {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
    #[shaku(inject)]
    host: Arc<dyn RecordHost>,
}

impl Directory for DirectoryFacade {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }

    fn host(&self) -> Arc<dyn RecordHostPort> {
        self.host.clone()
    }

    fn serves(&self, share: &str) -> Result<Option<String>, HostError> {
        self.host.who_serves(share, self.clock.now_ms())
    }

    fn register(&self, decl: &AppDecl, origin: &str) -> Result<Registered, HostError> {
        self.host.register(decl, origin)
    }
}

/// An admission decision, stamped with the instant it was made at: the
/// evidence frontier a re-evaluation compares against
/// (`dev-docs/arch1/RuntimeAndAssurance.md:54-60`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub at_ms: i64,
    pub outcome: Result<(), Denial>,
}

/// Admission: whether a holder may use a verb on a share, as the grant fold
/// answers at the injected clock's instant. No serve path consults it before
/// plan Step 4.3.
pub trait Admission: shaku::Interface {
    /// The clock this admission reads.
    fn clock(&self) -> Arc<dyn ClockPort>;

    /// The grant fold this admission consults.
    fn grants(&self) -> Arc<dyn GrantPort>;

    /// Decide now.
    fn admit(&self, holder: &Holder, verb: &str, share: &str) -> Decision;
}

#[derive(Component)]
#[shaku(interface = Admission)]
pub struct GrantAdmission {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
    #[shaku(inject)]
    grants: Arc<dyn Grants>,
}

impl Admission for GrantAdmission {
    fn clock(&self) -> Arc<dyn ClockPort> {
        self.clock.clone()
    }

    fn grants(&self) -> Arc<dyn GrantPort> {
        self.grants.clone()
    }

    fn admit(&self, holder: &Holder, verb: &str, share: &str) -> Decision {
        let at_ms = self.clock.now_ms();
        let outcome = self.grants.check(holder, verb, share);
        Decision { at_ms, outcome }
    }
}

/// Sessions: the consumer of both carrier roles, each through its own binding.
/// The Server still runs sessions over its own transports; they move here in
/// plan Phase 4.
pub trait Sessions: shaku::Interface {
    /// The peer role's carrier.
    fn peer(&self) -> Arc<dyn CarrierPort>;

    /// The client role's carrier.
    fn client(&self) -> Arc<dyn CarrierPort>;
}

#[derive(Component)]
#[shaku(interface = Sessions)]
pub struct RoleSessions {
    #[shaku(inject)]
    peer: Arc<dyn PeerCarrier>,
    #[shaku(inject)]
    client: Arc<dyn ClientCarrier>,
}

impl Sessions for RoleSessions {
    fn peer(&self) -> Arc<dyn CarrierPort> {
        self.peer.clone()
    }

    fn client(&self) -> Arc<dyn CarrierPort> {
        self.client.clone()
    }
}
