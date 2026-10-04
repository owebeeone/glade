use crate::*;
use glade_wire::{cbor::Cbor, generated::Op};
use std::collections::BTreeMap;
impl Canonical for u64 {
    const KIND: &'static str = "u64";
    fn encode_value(&self) -> Cbor {
        Cbor::Bytes(self.to_be_bytes().to_vec())
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Bytes(bytes) = value else {
            return Err(Error::Invalid);
        };
        Ok(u64::from_be_bytes(
            bytes.as_slice().try_into().map_err(|_| Error::Invalid)?,
        ))
    }
}
impl Canonical for usize {
    const KIND: &'static str = "usize";
    fn encode_value(&self) -> Cbor {
        (*self as u64).encode_value()
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        usize::try_from(u64::decode_value(value)?).map_err(|_| Error::Capacity)
    }
}
impl Canonical for u8 {
    const KIND: &'static str = "u8";
    fn encode_value(&self) -> Cbor {
        Cbor::Int(i64::from(*self))
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        u8::try_from(i64::decode_value(value)?).map_err(|_| Error::Invalid)
    }
}
impl Canonical for i64 {
    const KIND: &'static str = "i64";
    fn encode_value(&self) -> Cbor {
        Cbor::Int(*self)
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        if let Cbor::Int(v) = value {
            Ok(*v)
        } else {
            Err(Error::Invalid)
        }
    }
}
impl Canonical for bool {
    const KIND: &'static str = "bool";
    fn encode_value(&self) -> Cbor {
        Cbor::Bool(*self)
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        if let Cbor::Bool(v) = value {
            Ok(*v)
        } else {
            Err(Error::Invalid)
        }
    }
}
impl Canonical for String {
    const KIND: &'static str = "String";
    fn encode_value(&self) -> Cbor {
        Cbor::Text(self.clone())
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        if let Cbor::Text(v) = value {
            Ok(v.clone())
        } else {
            Err(Error::Invalid)
        }
    }
}
impl Canonical for Digest {
    const KIND: &'static str = "Digest";
    fn encode_value(&self) -> Cbor {
        Cbor::Bytes(self.to_vec())
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Bytes(v) = value else {
            return Err(Error::Invalid);
        };
        v.as_slice().try_into().map_err(|_| Error::Invalid)
    }
}
impl Canonical for Bytes {
    const KIND: &'static str = "Bytes";
    fn encode_value(&self) -> Cbor {
        Cbor::Bytes(self.value().to_vec())
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        if let Cbor::Bytes(v) = value {
            Ok(Bytes::new(v.clone()))
        } else {
            Err(Error::Invalid)
        }
    }
}
impl Canonical for StoreIncarnation {
    const KIND: &'static str = "StoreIncarnation";
    fn encode_value(&self) -> Cbor {
        self.0.encode_value()
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        Ok(Self(Digest::decode_value(value)?))
    }
}
impl<T: Canonical> Canonical for Vec<T> {
    const KIND: &'static str = "Vec<T>";
    fn root_identity() -> Cbor {
        Cbor::Array(vec![Cbor::Text("Vec".into()), T::root_identity()])
    }
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(values) = value else {
            return Err(Error::Invalid);
        };
        for value in values {
            T::preflight_value(value, budget, depth + 1)?;
        }
        Ok(())
    }
    fn encode_value(&self) -> Cbor {
        Cbor::Array(self.iter().map(Canonical::encode_value).collect())
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(v) = value else {
            return Err(Error::Invalid);
        };
        v.iter().map(T::decode_value).collect()
    }
}
impl<T: Canonical> Canonical for Option<T> {
    const KIND: &'static str = "Option<T>";
    fn root_identity() -> Cbor {
        Cbor::Array(vec![Cbor::Text("Option".into()), T::root_identity()])
    }
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        match value {
            Cbor::Null => Ok(()),
            Cbor::Array(v) if v.len() == 1 => T::preflight_value(&v[0], budget, depth + 1),
            _ => Err(Error::Invalid),
        }
    }
    fn encode_value(&self) -> Cbor {
        self.as_ref()
            .map_or(Cbor::Null, |v| Cbor::Array(vec![v.encode_value()]))
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        match value {
            Cbor::Null => Ok(None),
            Cbor::Array(v) if v.len() == 1 => Ok(Some(T::decode_value(&v[0])?)),
            _ => Err(Error::Invalid),
        }
    }
}
impl<T: Canonical> Canonical for Box<T> {
    const KIND: &'static str = "Box<T>";
    fn root_identity() -> Cbor {
        Cbor::Array(vec![Cbor::Text("Box".into()), T::root_identity()])
    }
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        T::preflight_value(value, budget, depth)
    }
    fn encode_value(&self) -> Cbor {
        (**self).encode_value()
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        Ok(Box::new(T::decode_value(value)?))
    }
}
impl<A: Canonical, B: Canonical> Canonical for (A, B) {
    const KIND: &'static str = "(A,B)";
    fn root_identity() -> Cbor {
        Cbor::Array(vec![
            Cbor::Text("Tuple2".into()),
            A::root_identity(),
            B::root_identity(),
        ])
    }
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(v) = value else {
            return Err(Error::Invalid);
        };
        if v.len() != 2 {
            return Err(Error::Invalid);
        }
        A::preflight_value(&v[0], budget, depth + 1)?;
        B::preflight_value(&v[1], budget, depth + 1)
    }
    fn encode_value(&self) -> Cbor {
        Cbor::Array(vec![self.0.encode_value(), self.1.encode_value()])
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(v) = value else {
            return Err(Error::Invalid);
        };
        if v.len() != 2 {
            return Err(Error::Invalid);
        }
        Ok((A::decode_value(&v[0])?, B::decode_value(&v[1])?))
    }
}
impl<K: Canonical + Ord, V: Canonical> Canonical for BTreeMap<K, V> {
    const KIND: &'static str = "BTreeMap<K,V>";
    fn root_identity() -> Cbor {
        Cbor::Array(vec![
            Cbor::Text("BTreeMap".into()),
            K::root_identity(),
            V::root_identity(),
        ])
    }
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Array(v) = value else {
            return Err(Error::Invalid);
        };
        for pair in v {
            let Cbor::Array(p) = pair else {
                return Err(Error::Invalid);
            };
            if p.len() != 2 {
                return Err(Error::Invalid);
            }
            K::preflight_value(&p[0], budget, depth + 2)?;
            V::preflight_value(&p[1], budget, depth + 2)?;
        }
        Ok(())
    }
    fn encode_value(&self) -> Cbor {
        Cbor::Array(
            self.iter()
                .map(|(k, v)| Cbor::Array(vec![k.encode_value(), v.encode_value()]))
                .collect(),
        )
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let Cbor::Array(v) = value else {
            return Err(Error::Invalid);
        };
        let mut out = Self::new();
        for p in v {
            let Cbor::Array(p) = p else {
                return Err(Error::Invalid);
            };
            if p.len() != 2 {
                return Err(Error::Invalid);
            }
            let key = K::decode_value(&p[0])?;
            if out
                .last_key_value()
                .is_some_and(|(previous, _)| previous >= &key)
            {
                return Err(Error::Invalid);
            }
            out.insert(key, V::decode_value(&p[1])?);
        }
        Ok(out)
    }
}
impl Canonical for Operation {
    const KIND: &'static str = "Operation";
    fn preflight_value(
        value: &Cbor,
        budget: &mut DecodeBudget<'_>,
        depth: usize,
    ) -> Result<(), Error> {
        let Cbor::Bytes(bytes) = value else {
            return Err(Error::Invalid);
        };
        budget.scan(bytes, depth.checked_add(1).ok_or(Error::Capacity)?)
    }
    fn encode_value(&self) -> Cbor {
        self.canonical().encode_value()
    }
    fn decode_value(value: &Cbor) -> Result<Self, Error> {
        let bytes = Bytes::decode_value(value)?;
        let op = Op::decode(bytes.value()).map_err(|_| Error::Invalid)?;
        let out = Self::new(op);
        if out.canonical() != &bytes {
            return Err(Error::Invalid);
        }
        Ok(out)
    }
}
