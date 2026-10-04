//! Frozen remote profile maps. No recovery magic and no authentication assertion.
use crate::*;
use glade_wire::cbor::{self, Cbor};
pub trait ProfileBody: Canonical {
    const CONTEXT: &'static str;
}
macro_rules! profile {($($name:ident),* $(,)?)=>{$(impl ProfileBody for $name {const CONTEXT: &'static str=stringify!($name);})*};}
profile!(
    CreationIntent,
    SignedDescriptorBody,
    CertificateBody,
    PolicyBody,
    PermitBody,
    OperationProofBody,
    PermitWrapper,
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
pub fn encode_profile<T: ProfileBody>(value: &T, limits: &DecodeLimits) -> Result<Bytes, Error> {
    let Cbor::Map(fields) = value.encode_value() else {
        return Err(Error::Invalid);
    };
    let mut map = vec![(0, Cbor::Int(1)), (1, Cbor::Text(T::CONTEXT.into()))];
    map.extend(fields.into_iter().map(|(key, value)| (key + 2, value)));
    let bytes = cbor::encode(&Cbor::Map(map));
    crate::bounds::check(&bytes, limits)?;
    Ok(Bytes::new(bytes))
}
pub fn decode_profile<T: ProfileBody>(bytes: &[u8], limits: &DecodeLimits) -> Result<T, Error> {
    let mut budget = DecodeBudget::new(limits);
    budget.scan(bytes, 0)?;
    let value = cbor::try_decode_with(bytes, limits.depth, Some(limits.bytes))
        .map_err(|_| Error::Invalid)?;
    if cbor::encode(&value) != bytes {
        return Err(Error::Invalid);
    }
    let Cbor::Map(map) = value else {
        return Err(Error::Invalid);
    };
    if map.iter().enumerate().any(|(i, (key, _))| *key != i as i64)
        || map.first() != Some(&(0, Cbor::Int(1)))
        || map.get(1) != Some(&(1, Cbor::Text(T::CONTEXT.into())))
    {
        return Err(Error::Invalid);
    }
    let value = Cbor::Map(
        map.into_iter()
            .skip(2)
            .map(|(key, value)| (key - 2, value))
            .collect(),
    );
    T::preflight_value(&value, &mut budget, 0)?;
    T::decode_value(&value)
}
