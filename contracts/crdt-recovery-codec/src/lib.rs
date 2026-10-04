//! Versioned bounded canonical representations. Not auth or physical durability.
pub use glade_crdt_admission_data::*;
use glade_wire::cbor::{self, Cbor};
mod batch;
mod bounds;
pub use bounds::DecodeBudget;
mod profile;
mod proofs;
mod schema;
mod values;
pub use batch::{DecodedBatch, decode_batch};
pub use profile::{ProfileBody, decode_profile, encode_profile};
pub use proofs::{ProofBody, decode_signed_body};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    Capacity,
}
/// INTERNAL fixed representation; concrete decoded images remain untrusted.
pub trait Canonical: Sized {
    const KIND: &'static str;
    /// Deterministic portable concrete identity; generic roots recursively bind arguments.
    fn root_identity() -> Cbor {
        Cbor::Text(Self::KIND.into())
    }
    /// Walk interpreted embedded syntax before allocating any typed fields.
    fn preflight_value(
        _value: &Cbor,
        _budget: &mut DecodeBudget<'_>,
        _depth: usize,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn encode_value(&self) -> Cbor;
    fn decode_value(value: &Cbor) -> Result<Self, Error>;
}
const DOMAIN: &[u8] = b"glade.crdt.recovery\0\x01";
pub fn encode<T: Canonical>(value: &T, limits: &DecodeLimits) -> Result<Bytes, Error> {
    let mut bytes = DOMAIN.to_vec();
    bytes.extend(cbor::encode(&Cbor::Array(vec![
        T::root_identity(),
        value.encode_value(),
    ])));
    bounds::check(&bytes[DOMAIN.len()..], limits)?;
    if bytes.len() > limits.bytes {
        return Err(Error::Capacity);
    }
    Ok(Bytes::new(bytes))
}
pub fn decode<T: Canonical>(bytes: &[u8], limits: &DecodeLimits) -> Result<T, Error> {
    if bytes.len() > limits.bytes {
        return Err(Error::Capacity);
    }
    let bytes = bytes.strip_prefix(DOMAIN).ok_or(Error::Invalid)?;
    let mut budget = DecodeBudget::new(limits);
    budget.scan(bytes, 0)?;
    let value = cbor::try_decode_with(bytes, limits.depth, Some(limits.bytes))
        .map_err(|_| Error::Invalid)?;
    if cbor::encode(&value) != bytes {
        return Err(Error::Invalid);
    }
    let Cbor::Array(envelope) = value else {
        return Err(Error::Invalid);
    };
    if envelope.len() != 2 || envelope[0] != T::root_identity() {
        return Err(Error::Invalid);
    }
    T::preflight_value(&envelope[1], &mut budget, 1)?;
    T::decode_value(&envelope[1])
}
macro_rules! record {
 ($name:ident; $($field:ident),* $(,)?) => {record!(@checked $name; $($field),*; |_: &$name| true);};
 (@checked $name:ident; $($field:ident),*; $valid:expr) => {
 impl Canonical for $name {
 const KIND: &'static str = stringify!($name);
 fn preflight_value(value:&Cbor,budget:&mut DecodeBudget<'_>,depth:usize)->Result<(),Error> {
 let Cbor::Map(values)=value else { return Err(Error::Invalid); };
 if values.iter().enumerate().any(|(index,(key,_))|*key!=index as i64) {return Err(Error::Invalid);}
 let mut fields=values.iter().map(|(_,value)|value);
 $(preflight_field(|record:&Self|&record.$field,fields.next().ok_or(Error::Invalid)?,budget,depth+1)?;)*
 if fields.next().is_some() { return Err(Error::Invalid); } Ok(())
 }
 fn encode_value(&self)->Cbor {
 let fields=vec![$(self.$field.encode_value()),*];
 Cbor::Map(fields.into_iter().enumerate().map(|(index,value)|(index as i64,value)).collect())
 }
 fn decode_value(value:&Cbor)->Result<Self,Error> {
 let Cbor::Map(values)=value else { return Err(Error::Invalid); };
 if values.iter().enumerate().any(|(index,(key,_))|*key!=index as i64) {return Err(Error::Invalid);}
 let mut fields=values.iter().map(|(_,value)|value);
 let out=Self { $($field: Canonical::decode_value(fields.next().ok_or(Error::Invalid)?)?),* };
 if fields.next().is_some() || !($valid)(&out) { return Err(Error::Invalid); } Ok(out)
 }
 }
 };
}
use record;

fn preflight_field<R, T: Canonical>(
    _: fn(&R) -> &T,
    value: &Cbor,
    budget: &mut DecodeBudget<'_>,
    depth: usize,
) -> Result<(), Error> {
    T::preflight_value(value, budget, depth)
}
