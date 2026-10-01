//! The grant check at the serve hop (plan Step 4.3): the node's grant fold as
//! the `GrantPort` its serve paths ask. The design, with each path and its
//! refusal, is `glade/dev-docs/GladeNodeAssembly.md`, "Grant check at the serve
//! hop (plan Step 4.3)".
//!
//! The fold is the registry's, records.json's: this node's own grants and
//! revocations, from its app files' `seed` and `revoke` lines. A peer's `home`
//! records reach the served store, never the registry, so a grant made on
//! another node admits nothing here (node trust, SP-T1). `GrantPort::check`
//! must not block, and the registry sits behind the directory's async lock,
//! so the serve paths read a [`PolicyView`]: the fold's grants and
//! revocations, replaced whenever the fold changes, with a generation that
//! counts the changes. A view with no fold answers `Unavailable`, and every
//! check fails closed: a node that has adopted no instance (the legacy form,
//! or a served store seeded without one), or one whose load quarantined a
//! grant or a revocation.
//!
//! Holders and verbs (ruled 2026-09-24). A grant names a node by its node id in
//! lower-case hex, and a principal by its name. A principal written as 64
//! lower-case hex digits would read as a node's grant, so it matches nothing:
//! a `Node` never matches a `Principal`. A read asks [`READ_SUBSCRIBE`], a
//! write [`WRITE_APPEND`] (cross-node writes plan X4.1), an exchange its own
//! glade id, and a granted `p.*` admits every verb that begins `p.`
//! (`glade_grant_api::admits`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{PoisonError, RwLock};

use glade_grant_api::{admits, Denial, GrantPort, Holder};

use crate::mesh::hex_id;

/// The verb a read asks for: a subscribe, whether a peer's or a client's.
pub const READ_SUBSCRIBE: &str = "read.subscribe";

/// The verb a write asks for (cross-node writes plan X4.1, question 6): a
/// forwarding node's, at the claim holder, or a client's, at its node while
/// client sessions are checked.
pub const WRITE_APPEND: &str = "write.append";

/// What a start prints when the fold it loaded cannot be read (plan Step 4.3):
/// a grant or a revocation was quarantined, so every grant check refuses.
pub const GRANTS_UNAVAILABLE: &str =
    "grants unavailable: a grant or revocation record was quarantined at load, so every grant check refuses";

/// What a start prints when it checks client sessions too, as switched on by
/// `--enforce-client-grants` (plan Step 4.3; off by default), their writes
/// included since cross-node writes plan X4.1.
pub const CLIENT_GRANTS_ENFORCED: &str =
    "client grants enforced: a client session reads or writes a share other than home only with a grant";

/// Whether `name` is written as a node's id: 64 lower-case hex digits.
pub fn names_a_node(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A fold of grants and revocations: per (principal, share), the union of the
/// verbs granted, and whether any revocation names the pair. A revocation
/// wins over every grant of its pair, made before it or after.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    granted: BTreeMap<(String, String), BTreeSet<String>>,
    revoked: BTreeSet<(String, String)>,
}

impl Policy {
    /// Fold in a grant of `verbs` to `principal` on `share`.
    pub fn grant(&mut self, principal: &str, share: &str, verbs: impl IntoIterator<Item = String>) {
        let pair = (principal.to_string(), share.to_string());
        self.granted.entry(pair).or_default().extend(verbs);
    }

    /// Fold in a revocation of `principal` on `share`.
    pub fn revoke(&mut self, principal: &str, share: &str) {
        self.revoked
            .insert((principal.to_string(), share.to_string()));
    }

    fn check(&self, principal: &str, verb: &str, share: &str) -> Result<(), Denial> {
        let pair = (principal.to_string(), share.to_string());
        if self.revoked.contains(&pair) {
            return Err(Denial::Revoked);
        }
        let verbs = self.granted.get(&pair);
        if verbs.is_some_and(|verbs| verbs.iter().any(|granted| admits(granted, verb))) {
            return Ok(());
        }
        Err(Denial::NoGrant)
    }
}

/// The name a grant gives `holder`: a node's id in hex, or a principal's name.
/// A principal whose name is written as a node's id names none.
fn grant_name(holder: &Holder) -> Option<String> {
    match holder {
        Holder::Node(id) => Some(hex_id(id)),
        Holder::Principal(name) if names_a_node(name) => None,
        Holder::Principal(name) => Some(name.clone()),
    }
}

/// The grant fold as the serve paths read it: the `GrantPort` adapter over the
/// node's fold (LBT-009). It holds the last fold it was given, or none, and
/// the generation, which each replacement advances. A check reads the fold as
/// it is when asked, never a decision made before a replacement.
pub struct PolicyView {
    state: RwLock<View>,
}

struct View {
    generation: u64,
    fold: Option<Policy>,
}

impl PolicyView {
    /// A view with no fold: every check is `Unavailable` (GR-003).
    pub fn unavailable() -> PolicyView {
        PolicyView::of(None)
    }

    /// A view of `fold`, or of none: a fold that cannot be read.
    pub fn of(fold: Option<Policy>) -> PolicyView {
        let state = RwLock::new(View {
            generation: 0,
            fold,
        });
        PolicyView { state }
    }

    /// Replace the fold, or leave none, and return the new generation.
    pub fn replace(&self, fold: Option<Policy>) -> u64 {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.generation += 1;
        state.fold = fold;
        state.generation
    }

    /// How many times the fold has been replaced.
    pub fn generation(&self) -> u64 {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
    }
}

impl GrantPort for PolicyView {
    fn check(&self, holder: &Holder, verb: &str, share: &str) -> Result<(), Denial> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let Some(fold) = &state.fold else {
            return Err(Denial::Unavailable);
        };
        let Some(name) = grant_name(holder) else {
            return Err(Denial::NoGrant);
        };
        fold.check(&name, verb, share)
    }
}

/// What a refusal says to a client session that names no principal, which
/// holds nothing (ruled 2026-09-24).
pub fn no_principal(verb: &str, share: &str) -> String {
    format!("unauthorized: a session that names no principal holds no grant of {verb} on {share}")
}

/// What a refusal says: who asked for which verb on which share, and why it
/// was refused. It names the holder as its session established it.
pub fn refusal(holder: &Holder, verb: &str, share: &str, denial: Denial) -> String {
    let who = match holder {
        Holder::Node(id) => format!("node {}", hex_id(id)),
        Holder::Principal(name) => format!("principal {name}"),
    };
    match denial {
        Denial::NoGrant => format!("unauthorized: {who} holds no grant of {verb} on {share}"),
        Denial::Revoked => format!("unauthorized: {who}'s grants on {share} are revoked"),
        Denial::Unavailable => {
            format!(
                "unauthorized: the grant fold is unavailable, so {who} may not {verb} on {share}"
            )
        }
    }
}
