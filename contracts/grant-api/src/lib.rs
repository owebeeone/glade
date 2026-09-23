//! The admission facade at its smallest: does this node or principal hold this
//! verb on this share? It is the question `RegistryApi::grants_for` answers over
//! the home-share fold (`glade/node/src/registry.rs`), asked as one typed
//! decision. Not authentication, binding resolution, trust policy or policy
//! composition.

/// Who is asking, as the session established it: never a name read from a wire DTO.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Holder {
    /// A node, by its 32-byte node id (`sha256` of its node key).
    Node([u8; 32]),
    /// A principal, by name.
    Principal(String),
}

/// Why a check refused. Every variant fails closed: nothing is served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    /// No grant in the fold gives this holder this verb on this share.
    NoGrant,
    /// The fold holds a revocation for this holder on this share.
    Revoked,
    /// The fold could not be read. This is not a finding that no grant exists.
    Unavailable,
}

/// `Ok(())` only when the fold grants `verb` on `share` to `holder` and holds no
/// revocation for that holder on that share.
///
/// Grants fold as a set union. A revocation denies its (holder, share) pair as
/// `Revoked` whatever was granted, in whichever order the fold saw the grant and
/// the revocation, and touches no other pair: revocation wins, as in
/// `grants_for`. Matching is exact. No verb implies another, no share another,
/// and no holder another's grants: a node does not hold its operator's, and a
/// `Node` never matches a `Principal`.
///
/// Each check reads the fold as it is when called, so a revocation denies the
/// next check; an implementation MUST NOT answer from a decision cached across
/// fold changes. It answers from state it already holds and never blocks on I/O;
/// a fold it cannot read is `Unavailable`. A grant is permission only: the caller
/// still authenticates the holder and validates what it serves.
///
/// ```compile_fail
/// use glade_grant_api::GrantPort;
/// struct Missing;
/// impl GrantPort for Missing {}
/// ```
///
/// It bridges onto an injector's facade with `'static` asked of the
/// implementation (`Interface` is `shaku::Interface` as Shaku defines it):
///
/// ```
/// use glade_grant_api::{Denial, GrantPort, Holder};
/// use std::any::Any;
/// trait Interface: Any + Send + Sync {}
/// impl<T: Any + Send + Sync> Interface for T {}
/// trait Grants: GrantPort + Interface {}
/// impl<T: GrantPort + 'static> Grants for T {}
/// fn is_interface<I: Interface + ?Sized>() {}
/// is_interface::<dyn Grants>();
/// fn build<T: GrantPort + 'static>(provider: T) -> Box<dyn Grants> { Box::new(provider) }
/// fn ask(grants: &dyn Grants, who: &Holder) -> Result<(), Denial> { grants.check(who, "read", "ws") }
/// ```
///
/// The witness's form does not compile, because the port names no `Any`:
///
/// ```compile_fail,E0310
/// # use glade_grant_api::GrantPort;
/// # trait Interface: std::any::Any + Send + Sync {}
/// # impl<T: std::any::Any + Send + Sync> Interface for T {}
/// trait Grants: GrantPort + Interface {}
/// impl<T: GrantPort> Grants for T {}
/// ```
pub trait GrantPort: Send + Sync {
    fn check(&self, holder: &Holder, verb: &str, share: &str) -> Result<(), Denial>;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Probes over one fixture fold; an adapter loads [`fold`] into its own
    //! store as records, in order.
    use crate::{Denial, GrantPort, Holder};

    /// One record of the fixture fold.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Record {
        Grant {
            holder: Holder,
            share: &'static str,
            verbs: &'static [&'static str],
        },
        Revoke {
            holder: Holder,
            share: &'static str,
        },
    }

    /// The node the fixture fold grants to.
    pub const NODE: [u8; 32] = [7; 32];

    fn principal(name: &str) -> Holder {
        Holder::Principal(name.into())
    }

    /// The fold GR-001 and GR-002 require, folded in this order.
    pub fn fold() -> Vec<Record> {
        let grant = |holder, share, verbs| Record::Grant {
            holder,
            share,
            verbs,
        };
        let revoke = |holder, share| Record::Revoke { holder, share };
        vec![
            grant(principal("alice"), "ws-a", &["read", "write"]),
            grant(principal("alice"), "ws-a", &["read"]),
            grant(Holder::Node(NODE), "ws-a", &["read"]),
            grant(principal("eve"), "ws-a", &["read", "write"]),
            revoke(principal("eve"), "ws-a"),
            revoke(principal("mallory"), "ws-a"),
            grant(principal("mallory"), "ws-a", &["read"]),
            grant(principal("eve"), "ws-b", &["read"]),
        ]
    }

    /// GR-001. Exact match on holder, verb and share; nothing is implied.
    pub fn exact(port: &dyn GrantPort) {
        let alice = principal("alice");
        for verb in ["read", "write"] {
            assert_eq!(
                port.check(&alice, verb, "ws-a"),
                Ok(()),
                "GR-001 folded grants admit"
            );
        }
        let node = Holder::Node(NODE);
        assert_eq!(
            port.check(&node, "read", "ws-a"),
            Ok(()),
            "GR-001 a node's own grant"
        );
        let refused = [
            (alice.clone(), "admin", "ws-a"),
            (alice, "read", "ws-b"),
            (principal("bob"), "read", "ws-a"),
            (Holder::Node([8; 32]), "read", "ws-a"),
            (node, "write", "ws-a"),
        ];
        for (holder, verb, share) in refused {
            let answer = port.check(&holder, verb, share);
            let message = format!("GR-001 nothing is implied: {holder:?} {verb} {share}");
            assert_eq!(answer, Err(Denial::NoGrant), "{message}");
        }
    }

    /// GR-002. Revocation wins over every verb of its pair, in either fold order,
    /// and only there.
    pub fn revocation_wins(port: &dyn GrantPort) {
        for verb in ["read", "write"] {
            let answer = port.check(&principal("eve"), verb, "ws-a");
            assert_eq!(
                answer,
                Err(Denial::Revoked),
                "GR-002 a revocation after the grant"
            );
        }
        let answer = port.check(&principal("mallory"), "read", "ws-a");
        assert_eq!(
            answer,
            Err(Denial::Revoked),
            "GR-002 a revocation before the grant"
        );
        let answer = port.check(&principal("eve"), "read", "ws-b");
        assert_eq!(
            answer,
            Ok(()),
            "GR-002 a revocation touches only its own pair"
        );
    }

    /// GR-003. Requires a fold configured unreadable: the answer is `Unavailable`,
    /// never a grant and never a finding that no grant exists.
    pub fn unavailable(port: &dyn GrantPort) {
        let answer = port.check(&principal("alice"), "read", "ws-a");
        assert_eq!(
            answer,
            Err(Denial::Unavailable),
            "GR-003 an unreadable fold"
        );
    }
}
