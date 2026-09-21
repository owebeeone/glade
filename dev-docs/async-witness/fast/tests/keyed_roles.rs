//! **Step 1.5 — keyed roles.** The binding graph gives one contract two
//! recipes and then requires two of them to share one occurrence
//! (`InjectionGraphRefinement.md:15-17,23`): `peer_carrier_binding` selects the
//! IrohAdapter, `client_carrier_binding` selects the WebSocketAdapter, and
//! `record_transport_binding` is bound to the **same IrohAdapter occurrence**.
//! "The binding occurrence distinguishes a role; matching a port type alone
//! does not."
//!
//! What is witnessed here and what is not:
//!
//! - **Witnessed.** Two occurrences of one contract are distinguished by a
//!   typed key, not by a name; they are genuinely distinct providers; a third
//!   recipe receives the very same occurrence the peer recipe selected; and a
//!   role no recipe registered is an absent map entry rather than a panic —
//!   which is DI-E03's remaining positive half, the run-time counterpart of
//!   `examples/ambiguous_role.rs`.
//! - **Not witnessed.** `record_transport_binding`'s contract is `TransportPort`,
//!   and the witness declares no such port, so what is shown is the *sharing*
//!   requirement rather than a second contract over one provider. The real
//!   providers are an iroh endpoint and a WebSocket, neither of which can
//!   compile into this crate; these are stand-ins that differ only in the frame
//!   they replay.
//! - **Not claimed.** DI-E06 is out of scope for this witness
//!   (`AsyncWitnessPlan.md` §10). This is incidental evidence toward it and is
//!   reported as Step 1.5, nothing more.

use std::collections::HashMap;
use std::sync::Arc;

use async_witness_fast::{Carrier, CarrierRole, KeyedComposition, RecordTransport, resolve_now};
use async_witness_ports::{CarrierPort, FrameType};
use shaku::{HasComponent, HasComponentMap};

fn occurrences(module: &KeyedComposition) -> &HashMap<CarrierRole, Arc<dyn Carrier>> {
    module.resolve_map()
}

#[test]
fn two_recipes_over_one_contract_select_two_distinct_providers() {
    let module = KeyedComposition::builder().build();
    let occurrences = occurrences(&module);

    let peer = occurrences
        .get(&CarrierRole::Peer)
        .expect("peer_carrier_binding");
    let client = occurrences
        .get(&CarrierRole::Client)
        .expect("client_carrier_binding");
    assert!(!Arc::ptr_eq(peer, client));

    // Distinct by behaviour too, read through the port with no downcast: each
    // stand-in replays the role it was registered for.
    assert_eq!(
        resolve_now(peer.recv()),
        Ok(Some((FrameType::NodeHello, b"peer".to_vec())))
    );
    assert_eq!(
        resolve_now(client.recv()),
        Ok(Some((FrameType::Hello, b"client".to_vec())))
    );
}

#[test]
fn a_role_no_recipe_registered_is_an_absent_entry_not_a_panic() {
    let module = KeyedComposition::builder().build();
    let occurrences = occurrences(&module);

    assert!(!occurrences.contains_key(&CarrierRole::Unbound));
    assert!(occurrences.get(&CarrierRole::Unbound).is_none());

    let transport: Arc<dyn RecordTransport> = module.resolve();
    assert!(transport.shared_with(CarrierRole::Unbound).is_none());
}

#[test]
fn the_record_transport_recipe_shares_the_peer_occurrence() {
    let module = KeyedComposition::builder().build();
    let transport: Arc<dyn RecordTransport> = module.resolve();
    let occurrences = occurrences(&module);

    for role in [CarrierRole::Peer, CarrierRole::Client] {
        let selected: Arc<dyn CarrierPort> =
            occurrences.get(&role).expect("a registered recipe").clone();
        let shared = transport
            .shared_with(role)
            .expect("the record transport recipe holds every registered occurrence");
        assert!(
            Arc::ptr_eq(&selected, &shared),
            "the record transport recipe did not share the {role:?} occurrence"
        );
    }

    assert_eq!(
        transport.roles(),
        vec![CarrierRole::Peer, CarrierRole::Client]
    );
}

/// One scope, one occurrence per role — the keyed map is resolved, not rebuilt.
#[test]
fn a_second_resolution_of_the_map_is_the_same_occurrence() {
    let module = KeyedComposition::builder().build();
    let first: Arc<dyn Carrier> = occurrences(&module)
        .get(&CarrierRole::Peer)
        .expect("peer_carrier_binding")
        .clone();
    let second: Arc<dyn Carrier> = occurrences(&module)
        .get(&CarrierRole::Peer)
        .expect("peer_carrier_binding")
        .clone();
    assert!(Arc::ptr_eq(&first, &second));
}
