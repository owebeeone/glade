use glade_crdt_admission_data::*;
use glade_crdt_evidence_api::*;
struct Refusing;
impl EvidencePort for Refusing {
    fn verify(&mut self, q: BoundQuery) -> EvidenceReply {
        EvidenceReply {
            query: q.query,
            verdict: Verdict::Unavailable,
        }
    }
    fn seal(&mut self, q: BoundQuery) -> EvidenceReply {
        EvidenceReply {
            query: q.query,
            verdict: Verdict::Unavailable,
        }
    }
    fn observe(&mut self, _: &Instance, _: &IdentityBinding) -> Result<EvidenceObservation, Fault> {
        Err(Fault::Unavailable)
    }
}
#[test]
fn concrete_provider_must_supply_all_three_trusted_operations() {
    fn consumer<T: EvidencePort>(_: &mut T) {}
    consumer(&mut Refusing);
}
#[test]
fn required_evidence_methods_have_no_default_provider() {
    let ast = syn::parse_file(include_str!("../src/lib.rs")).unwrap();
    let trait_item = ast
        .items
        .into_iter()
        .find_map(|item| {
            if let syn::Item::Trait(t) = item {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    let methods: Vec<_> = trait_item
        .items
        .into_iter()
        .filter_map(|item| {
            if let syn::TraitItem::Fn(method) = item {
                assert!(method.default.is_none());
                Some(method.sig.ident.to_string())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(methods, ["verify", "seal", "observe"]);
}
