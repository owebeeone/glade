//! Independent named-semantic input derivation through actual owned Rust types.
use glade_crdt_recovery_codec::*;
use glade_wire::{
    cbor::Cbor,
    generated::{Head, Op, Shape},
};
use serde_json::Value;
fn limits() -> DecodeLimits {
    DecodeLimits {
        bytes: 262144,
        items: 8192,
        depth: 64,
        name_bytes: 256,
    }
}
fn unhex(value: &str) -> Vec<u8> {
    assert!(value.len() % 2 == 0);
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
        .collect()
}
fn hex(value: &[u8]) -> String {
    value.iter().map(|v| format!("{v:02x}")).collect()
}
fn semantic(ty: &str, value: &Value, types: &Value) -> Cbor {
    match ty {
        "String" => return Cbor::Text(value.as_str().unwrap().into()),
        "Digest" | "Bytes" => {
            let bytes = unhex(value.as_str().unwrap());
            if ty == "Digest" {
                assert_eq!(bytes.len(), 32);
            }
            return Cbor::Bytes(bytes);
        }
        "u64" | "usize" => {
            return Cbor::Bytes(
                value
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
                    .to_be_bytes()
                    .to_vec(),
            );
        }
        "i64" | "u8" => return Cbor::Int(value.as_str().unwrap().parse().unwrap()),
        "bool" => return Cbor::Bool(value.as_bool().unwrap()),
        "Operation" => {
            let op = Op {
                share: value["share"].as_str().unwrap().into(),
                glade_id: value["glade_id"].as_str().unwrap().into(),
                key: unhex(value["key"].as_str().unwrap()),
                origin: value["origin"].as_str().unwrap().into(),
                seq: value["seq"].as_str().unwrap().parse().unwrap(),
                prev: value["prev"].as_str().map(unhex),
                lamport: value["lamport"].as_str().unwrap().parse().unwrap(),
                refs: value["refs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| Head {
                        origin: h["origin"].as_str().unwrap().into(),
                        seq: h["seq"].as_str().unwrap().parse().unwrap(),
                        hash: h["hash"].as_str().map(unhex),
                    })
                    .collect(),
                shape: Shape::from_wire(value["shape"].as_str().unwrap().parse().unwrap()).unwrap(),
                payload: unhex(value["payload"].as_str().unwrap()),
            };
            return Operation::new(op).canonical().encode_value();
        }
        _ => {}
    }
    if ty.starts_with("Vec<") {
        return Cbor::Array(
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| semantic(&ty[4..ty.len() - 1], v, types))
                .collect(),
        );
    }
    if ty.starts_with("Option<") {
        return if value.is_null() {
            Cbor::Null
        } else {
            Cbor::Array(vec![semantic(&ty[7..ty.len() - 1], value, types)])
        };
    }
    if ty.starts_with("Box<") {
        return semantic(&ty[4..ty.len() - 1], value, types);
    }
    let schema = &types[ty];
    if schema["kind"] == "enum" {
        let (index, variant) = schema["variants"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .find(|(_, entry)| entry[0] == value["variant"])
            .unwrap();
        let fields = variant[1].as_array().unwrap();
        assert_eq!(value["fields"].as_object().unwrap().len(), fields.len());
        let mut out = vec![Cbor::Int(index as i64)];
        out.extend(fields.iter().map(|field| {
            semantic(
                field[1].as_str().unwrap(),
                &value["fields"][field[0].as_str().unwrap()],
                types,
            )
        }));
        return Cbor::Array(out);
    }
    let fields = schema["fields"].as_array().unwrap();
    assert_eq!(value.as_object().unwrap().len(), fields.len());
    Cbor::Map(
        fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                (
                    index as i64,
                    semantic(
                        field[1].as_str().unwrap(),
                        &value[field[0].as_str().unwrap()],
                        types,
                    ),
                )
            })
            .collect(),
    )
}
fn check<T: ProfileBody + PartialEq + std::fmt::Debug>(body: &Cbor, expected: &Value) {
    let typed = T::decode_value(body).unwrap();
    let raw = encode_profile(&typed, &limits()).unwrap();
    assert_eq!(hex(raw.value()), expected["hex"].as_str().unwrap());
    assert_eq!(hex(&raw.digest()), expected["sha256"].as_str().unwrap());
    assert_eq!(decode_profile::<T>(raw.value(), &limits()).unwrap(), typed);
}
#[test]
fn all_remote_profile_categories_derive_exact_bytes_and_digests_from_semantic_inputs() {
    let corpus: Value = serde_json::from_str(include_str!("../vectors/input.json")).unwrap();
    let schema: Value = serde_json::from_str(include_str!("../vectors/schema.json")).unwrap();
    let expected: Value = serde_json::from_str(include_str!("../vectors/expected.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert!(
        cases.len() >= 21,
        "all remote namespace/proof/transfer categories required before IC3C"
    );
    let mut categories = std::collections::BTreeSet::new();
    for case in cases {
        let name = case["kind"].as_str().unwrap();
        categories.insert(name);
        let body = semantic(name, &case["value"], &schema["types"]);
        let bytes = &expected[case["id"].as_str().unwrap()];
        macro_rules! dispatch {($($ty:ident),*)=>{match name {$(stringify!($ty)=>check::<$ty>(&body,bytes),)* _=>panic!("missing typed vector category {name}")}};}
        dispatch!(
            CreationIntent,
            SignedDescriptorBody,
            CertificateBody,
            PermitBody,
            OperationProofBody,
            PermitWrapper,
            PolicyBody,
            LocalChallengeBody,
            LocalResponseBody,
            OriginalAdmissionBody,
            ExchangeHello,
            InventoryHeaderBody,
            Inventory,
            InventoryPage,
            RecordRef,
            TransferBundle,
            IdentityBinding,
            SignedRecord
        );
    }
    assert_eq!(categories.len(), 18);
    assert_eq!(expected.as_object().unwrap().len(), cases.len());
}
#[test]
fn bounded_malformed_canonical_vectors_fail_closed() {
    let negatives: Value = serde_json::from_str(include_str!("../vectors/negative.json")).unwrap();
    for case in negatives["cases"].as_array().unwrap() {
        let raw = unhex(case["hex"].as_str().unwrap());
        let refused = match case["kind"].as_str() {
            Some("SignedRecord") => decode_profile::<SignedRecord>(&raw, &limits()).is_err(),
            Some("SignedDescriptorBody") => {
                decode_profile::<SignedDescriptorBody>(&raw, &limits()).is_err()
            }
            None => decode_profile::<IdentityBinding>(&raw, &limits()).is_err(),
            Some(kind) => panic!("missing negative consumer {kind}"),
        };
        assert!(refused, "{}", case["id"]);
    }
}
#[test]
fn closed_maps_reject_unknown_duplicate_reordered_or_wrong_context_fields() {
    let descriptor = SignedDescriptorBody {
        intent: Bytes::new(vec![1, 2]),
        creation_root: [3; 32],
    };
    let bytes = encode_profile(&descriptor, &limits()).unwrap();
    let Cbor::Map(mut map) = glade_wire::cbor::try_decode(bytes.value()).unwrap() else {
        panic!("map");
    };
    map.push((4, Cbor::Null));
    assert!(
        decode_profile::<SignedDescriptorBody>(
            &glade_wire::cbor::encode(&Cbor::Map(map.clone())),
            &limits()
        )
        .is_err()
    );
    map.pop();
    map.swap(0, 1);
    let mut reordered = vec![0xa4];
    for (key, value) in &map {
        reordered.extend(glade_wire::cbor::encode(&Cbor::Int(*key)));
        reordered.extend(glade_wire::cbor::encode(value));
    }
    assert!(decode_profile::<SignedDescriptorBody>(&reordered, &limits()).is_err());
    map.swap(0, 1);
    map[1].1 = Cbor::Text("CreationIntent".into());
    assert!(
        decode_profile::<SignedDescriptorBody>(
            &glade_wire::cbor::encode(&Cbor::Map(map)),
            &limits()
        )
        .is_err()
    );
}
#[test]
fn malformed_signature_length_and_wrong_exact_domain_refuse_signed_envelope_shape() {
    let mut envelope = SignedRecord {
        purpose: DESCRIPTOR_DOMAIN.into(),
        version: 1,
        signer: [1; 32],
        body: Bytes::new(vec![2]),
        signature: Bytes::new(vec![3; 63]),
    };
    let raw = encode_profile(&envelope, &limits()).unwrap();
    assert!(decode_profile::<SignedRecord>(raw.value(), &limits()).is_err());
    envelope.signature = Bytes::new(vec![3; 64]);
    envelope.purpose = "glade/ic3/v1/descriptor".into();
    let raw = encode_profile(&envelope, &limits()).unwrap();
    assert!(decode_profile::<SignedRecord>(raw.value(), &limits()).is_err());
}
