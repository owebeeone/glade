//! Part (ii) of plan Step 3.4: wrong scope, and unknown or denied authority.

use std::sync::Arc;

use glade_grant_api::conformance::Record as GrantRecord;
use glade_grant_api::{Denial, Holder};
use glade_node::appdecl::parse;
use glade_node::assembly::{Admission, Decision, HostError};
use glade_node::registry::{Registry, RegistryApi, G_CLAIMS, HOME};
use glade_node::sysdata::SystemSnapshot;
use glade_wire::generated::Op;
use shaku::HasComponent;

use crate::fakes::{FakeClock, FakeNet};
use crate::faults::LiveGrants;
use crate::{pair, TestNode, T0, WS};

/// Wrong scope: B's record host takes only what its directory profile hosts,
/// the `home` share and its nine streams. An op carried on another share (a
/// foreign namespace) or on a stream the profile does not host is refused
/// before it is verified, and nothing is persisted, so B answers none until
/// the op arrives in scope. The fakes prove nothing about a referral (the
/// slice has none: its peers are named, SP-N1), a copied or forged op (ops
/// carry no signature, SP-P3(a)), or who may connect. Phase 4: 4.1's origin
/// signatures, 4.2's accept-time check.
#[test]
fn wrong_scope() {
    let (a, b) = pair(&FakeClock::at(T0));
    assert!(a.append(a.lease(WS, 1)));
    let claim = a.store.ops().remove(0);
    let foreign = Op {
        share: WS.into(),
        ..claim.clone()
    };
    let unhosted = Op {
        glade_id: "app.notes".into(),
        ..claim.clone()
    };

    assert_eq!(a.push(&[foreign, unhosted]), Ok(2));
    let refused = b.deliver();
    assert_eq!(refused.len(), 2);
    assert!(out_of_scope(&refused[0], WS, G_CLAIMS), "{refused:?}");
    assert!(out_of_scope(&refused[1], HOME, "app.notes"), "{refused:?}");
    assert_eq!(
        b.store.snapshot(),
        SystemSnapshot::default(),
        "nothing persisted"
    );
    assert_eq!(b.serves(WS), None);

    assert_eq!(a.push(&[claim]), Ok(1));
    assert!(b.deliver().iter().all(Result::is_ok));
    assert_eq!(b.serves(WS), Some("a".into()));
}

/// Whether `answer` refuses an op on `share` and `stream` as outside the
/// directory profile.
fn out_of_scope(answer: &Result<(), HostError>, share: &str, stream: &str) -> bool {
    let Err(HostError::OutOfScope {
        share: s,
        glade_id: g,
    }) = answer
    else {
        return false;
    };
    s == share && g == stream
}

/// A surface's declared authority on `node`, as the R9 fold (`bindings_of`)
/// reads what the node persisted: `(glade id, authority)`, live only.
fn declared(node: &TestNode) -> Vec<(String, String)> {
    let (fold, rejected) = Registry::from_snapshot(&node.store.snapshot());
    assert_eq!(rejected, 0);
    let live = fold.bindings_of().into_iter();
    live.map(|b| (b.glade_id, b.authority)).collect()
}

fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    let row = |(id, authority): &(&str, &str)| (id.to_string(), authority.to_string());
    rows.iter().map(row).collect()
}

/// Unknown or denied authority, in both senses the node has. A holder's
/// authority is a grant: admission asks the grant binding at the injected
/// clock; a holder the fold never granted is refused, since the fold keys by
/// holder and a grant passes to no other (AR-05's copied proof), not even
/// from an operator to its node; a revocation that lands between two
/// decisions denies the second, no decision being cached, and a later grant
/// does not undo it; a fold that cannot be read refuses. A surface's
/// authority is its declaration: an unknown authority token refuses the file,
/// and the R9 fold holds each surface's live declaration, `share` or
/// `external`. An app's retraction withdraws its own declaration only, and no
/// app holds a glade id: another app's later declaration stands. Nothing
/// enforces either before 4.3. The live fold proves no chain, issuer or
/// persistence. Phase 4: 4.3's grant adapter over the node's fold, consulted
/// at the serve hop.
#[test]
fn unknown_or_denied_authority() {
    let clock = FakeClock::at(T0);
    let grants = LiveGrants::fixture();
    let a = TestNode::new(&FakeNet::new(), &clock, "a", &[], grants.clone());
    let admission: Arc<dyn Admission> = a.module.resolve();
    let alice = Holder::Principal("alice".into());
    let admitted = Decision {
        at_ms: T0,
        outcome: Ok(()),
    };
    assert_eq!(admission.admit(&alice, "write", "ws-a"), admitted);
    for other in [Holder::Principal("bob".into()), Holder::Node([9; 32])] {
        let outcome = admission.admit(&other, "write", "ws-a").outcome;
        assert_eq!(
            outcome,
            Err(Denial::NoGrant),
            "{other:?} holds alice's grant"
        );
    }

    clock.advance(1);
    grants.load(GrantRecord::Revoke {
        holder: alice.clone(),
        share: "ws-a",
    });
    let denied = Decision {
        at_ms: T0 + 1,
        outcome: Err(Denial::Revoked),
    };
    assert_eq!(admission.admit(&alice, "write", "ws-a"), denied);
    grants.load(GrantRecord::Grant {
        holder: alice.clone(),
        share: "ws-a",
        verbs: &["write"],
    });
    assert_eq!(admission.admit(&alice, "write", "ws-a"), denied);
    grants.set_readable(false);
    let unread = admission.admit(&alice, "read", "ws-b").outcome;
    assert_eq!(unread, Err(Denial::Unavailable));

    let file = |app: &str, lines: &str| format!("glade-app v1\napp {app}\n{lines}");
    let line =
        |id: &str, authority: &str| format!("binding {id} value {authority} commons latest\n");
    let unknown = parse(&file("notes", &line("notes.list", "shared")));
    assert!(unknown.unwrap_err().contains("unknown authority `shared`"));

    let register = |app: &str, lines: &str| {
        let decl = parse(&file(app, lines)).unwrap();
        let registered = a.directory().register(&decl, "a").unwrap();
        (registered.appended, registered.unchanged)
    };
    let (list, feed) = (line("notes.list", "share"), line("notes.feed", "external"));
    assert_eq!(register("notes", &format!("{list}{feed}")), (2, 0));
    let notes = [("notes.feed", "external"), ("notes.list", "share")];
    assert_eq!(declared(&a), pairs(&notes));
    assert_eq!(register("other", &line("notes.list", "external")), (1, 0));
    let taken = [("notes.feed", "external"), ("notes.list", "external")];
    assert_eq!(declared(&a), pairs(&taken), "the later declaration stands");
    assert_eq!(register("notes", &feed), (1, 1), "notes retracts its own");
    assert_eq!(declared(&a), pairs(&taken), "other's declaration stays");
    assert_eq!(register("other", ""), (1, 0));
    assert_eq!(declared(&a), pairs(&[("notes.feed", "external")]));
}
