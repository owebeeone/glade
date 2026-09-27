//! Frame values the generated decode panics on, refused instead (F12, the
//! owner's ruling of 2026-09-27). `generated.rs` is regenerated from taut's
//! IR, and each enum's `from_wire` panics on a value the enum does not name,
//! so a frame holding one ended the task that read it. This module is
//! written by hand: each enum's `try_from_wire` takes the values its
//! `from_wire` takes, and [`frame_type`] and [`frame_body`] read a frame's
//! enum values before the generated decode does, so a reader refuses the
//! frame with an [`UnknownValue`] and goes on.
//!
//! It checks enum values only. A body of another shape (a missing field, a
//! value of the wrong type) and bytes that are not CBOR still panic in the
//! generated decode and in `cbor::decode`, as before.

use std::fmt;

use crate::cbor::Cbor;
use crate::generated::{ErrorCode, FrameType, Priority, Shape};

/// A frame's value that its enum does not name: the field, and the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownValue {
    pub field: &'static str,
    pub value: i64,
}

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad frame: unknown {} {}", self.field, self.value)
    }
}

impl FrameType {
    /// The frame type `v` names, or `None` where `from_wire` panics.
    pub fn try_from_wire(v: i64) -> Option<FrameType> {
        (0..=14).contains(&v).then(|| FrameType::from_wire(v))
    }
}

impl Shape {
    /// The shape `v` names, or `None` where `from_wire` panics.
    pub fn try_from_wire(v: i64) -> Option<Shape> {
        (0..=4).contains(&v).then(|| Shape::from_wire(v))
    }
}

impl Priority {
    /// The priority `v` names, or `None` where `from_wire` panics.
    pub fn try_from_wire(v: i64) -> Option<Priority> {
        (0..=2).contains(&v).then(|| Priority::from_wire(v))
    }
}

impl ErrorCode {
    /// The error code `v` names, or `None` where `from_wire` panics.
    pub fn try_from_wire(v: i64) -> Option<ErrorCode> {
        (0..=6).contains(&v).then(|| ErrorCode::from_wire(v))
    }
}

/// The type a frame's tag byte names, or the tag refused.
pub fn frame_type(tag: u8) -> Result<FrameType, UnknownValue> {
    let unknown = UnknownValue {
        field: "frame type",
        value: tag.into(),
    };
    FrameType::try_from_wire(tag.into()).ok_or(unknown)
}

/// Refuse a frame body of type `ty` that holds an enum value its enum does
/// not name: an op's shape or an `Ops` frame's priority, or an `Error`
/// frame's code, each read where the generated decode reads it. It never
/// panics: a body shaped otherwise is left to the generated decode.
pub fn frame_body(ty: FrameType, body: &Cbor) -> Result<(), UnknownValue> {
    match ty {
        FrameType::Ops => {
            for op in array_at(body, 1) {
                known("shape", int_at(op, 9), Shape::try_from_wire)?;
            }
            known("priority", int_at(body, 2), Priority::try_from_wire)
        }
        FrameType::Error => known("error code", int_at(body, 1), ErrorCode::try_from_wire),
        _ => Ok(()),
    }
}

/// `Err` when `value` is an int that `named` finds no value of its enum for.
fn known<T>(
    field: &'static str,
    value: Option<i64>,
    named: fn(i64) -> Option<T>,
) -> Result<(), UnknownValue> {
    match value {
        Some(value) if named(value).is_none() => Err(UnknownValue { field, value }),
        _ => Ok(()),
    }
}

/// The value at `key` of a map, as the generated decode reads it: the first
/// entry with that key.
fn at(c: &Cbor, key: i64) -> Option<&Cbor> {
    c.map_entries()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// The int at `key` of a map, if it is one.
fn int_at(c: &Cbor, key: i64) -> Option<i64> {
    match at(c, key) {
        Some(Cbor::Int(v)) => Some(*v),
        _ => None,
    }
}

/// The array at `key` of a map, or none.
fn array_at(c: &Cbor, key: i64) -> &[Cbor] {
    match at(c, key) {
        Some(Cbor::Array(items)) => items,
        _ => &[],
    }
}

// A braced module, so the condition encloses the whole section.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor;
    use crate::generated::{Error, Op, Ops, VECTORS};
    use std::panic;

    fn hex(h: &str) -> Vec<u8> {
        let byte = |i| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
        (0..h.len()).step_by(2).map(byte).collect()
    }

    /// A message's map with the value at `key` replaced by the int `value`.
    fn with(mut message: Cbor, key: i64, value: i64) -> Cbor {
        if let Cbor::Map(entries) = &mut message {
            for (k, v) in entries.iter_mut() {
                if *k == key {
                    *v = Cbor::Int(value);
                }
            }
        }
        message
    }

    /// An op whose shape value is `shape`.
    fn op(shape: i64) -> Cbor {
        with(Op::default().to_cbor(), 9, shape)
    }

    /// An `Ops` body of `ops`, with the priority `pri`.
    fn ops(ops: Vec<Cbor>, pri: Cbor) -> Cbor {
        Cbor::Map(vec![(1, Cbor::Array(ops)), (2, pri)])
    }

    /// Whether `from` panics on `v`.
    fn panics(from: fn(i64), v: i64) -> bool {
        panic::catch_unwind(move || from(v)).is_err()
    }

    /// Each enum's `try_from_wire` takes exactly the values its generated
    /// `from_wire` takes: every value it names round-trips, and it refuses
    /// the values past each end, where `from_wire` panics. So a value the IR
    /// adds or renumbers fails here until this module follows it.
    #[test]
    fn try_from_wire_takes_the_values_from_wire_takes() {
        let edges = |last: i64| [-1, last + 1, i64::MIN, i64::MAX];
        for v in 0..=14 {
            assert_eq!(FrameType::try_from_wire(v).map(FrameType::wire), Some(v));
        }
        for v in edges(14) {
            assert_eq!(FrameType::try_from_wire(v), None, "{v}");
            assert!(panics(|v| _ = FrameType::from_wire(v), v), "{v}");
        }
        for v in 0..=4 {
            assert_eq!(Shape::try_from_wire(v).map(Shape::wire), Some(v));
        }
        for v in edges(4) {
            assert_eq!(Shape::try_from_wire(v), None, "{v}");
            assert!(panics(|v| _ = Shape::from_wire(v), v), "{v}");
        }
        for v in 0..=2 {
            assert_eq!(Priority::try_from_wire(v).map(Priority::wire), Some(v));
        }
        for v in edges(2) {
            assert_eq!(Priority::try_from_wire(v), None, "{v}");
            assert!(panics(|v| _ = Priority::from_wire(v), v), "{v}");
        }
        for v in 0..=6 {
            assert_eq!(ErrorCode::try_from_wire(v).map(ErrorCode::wire), Some(v));
        }
        for v in edges(6) {
            assert_eq!(ErrorCode::try_from_wire(v), None, "{v}");
            assert!(panics(|v| _ = ErrorCode::from_wire(v), v), "{v}");
        }
    }

    /// F12: a frame whose tag, op shape, priority or error code names no
    /// value of its enum is refused, naming the field and the value, where
    /// the generated decode panicked; a later op of the frame is read too.
    /// A frame holding only named values passes, and so does every golden
    /// `Ops` and `Error` vector. A body shaped otherwise is not this check's
    /// to refuse, and does not panic it.
    #[test]
    fn a_frame_holding_an_unknown_enum_value_is_refused() {
        let unknown = |field, value| UnknownValue { field, value };
        assert_eq!(frame_type(15), Err(unknown("frame type", 15)));
        assert_eq!(frame_type(255), Err(unknown("frame type", 255)));
        assert_eq!(frame_type(4), Ok(FrameType::Ops));

        let (frame, null) = (FrameType::Ops, Cbor::Null);
        assert_eq!(
            frame_body(frame, &ops(vec![op(5)], null.clone())),
            Err(unknown("shape", 5))
        );
        assert_eq!(
            frame_body(frame, &ops(vec![op(-1)], null.clone())),
            Err(unknown("shape", -1))
        );
        let later = ops(vec![op(0), op(9)], null.clone());
        assert_eq!(frame_body(frame, &later), Err(unknown("shape", 9)));
        let pri = ops(vec![op(4)], Cbor::Int(3));
        assert_eq!(frame_body(frame, &pri), Err(unknown("priority", 3)));
        let named = ops(vec![op(4), op(0)], Cbor::Int(2));
        assert_eq!(frame_body(frame, &named), Ok(()));
        assert_eq!(Ops::from_cbor(&named).ops[0].shape, Shape::Crdt);

        let error = |code| with(Error::default().to_cbor(), 1, code);
        assert_eq!(
            frame_body(FrameType::Error, &error(7)),
            Err(unknown("error code", 7))
        );
        assert_eq!(frame_body(FrameType::Error, &error(6)), Ok(()));
        let elsewhere = ops(vec![op(9)], null);
        assert_eq!(frame_body(FrameType::Hello, &elsewhere), Ok(()));

        for (name, message, bytes) in VECTORS {
            let ty = match *message {
                "Ops" => FrameType::Ops,
                "Error" => FrameType::Error,
                _ => continue,
            };
            assert_eq!(frame_body(ty, &cbor::decode(&hex(bytes))), Ok(()), "{name}");
        }
        let odd = [
            Cbor::Int(3),
            Cbor::Map(vec![]),
            Cbor::Map(vec![(1, Cbor::Int(1))]),
        ];
        for body in odd {
            assert_eq!(frame_body(frame, &body), Ok(()), "{body:?}");
        }
        let shown = UnknownValue {
            field: "shape",
            value: 9,
        };
        assert_eq!(shown.to_string(), "bad frame: unknown shape 9");
    }
}
