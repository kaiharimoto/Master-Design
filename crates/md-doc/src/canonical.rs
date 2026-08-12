//! Canonical serialization.
//!
//! Two editors write these files: a person dragging handles, and a model applying
//! patches. If the same document state could serialize two different ways, every
//! handoff between them would produce a diff full of noise — reordered keys, `4.0`
//! versus `4`, `0.30000000000000004` where the user typed `0.3` — and the git history
//! would stop being readable. Worse, the two would start fighting: each save would
//! revert the other's formatting.
//!
//! So there is exactly one byte sequence for a given document:
//!
//! * **Keys sorted.** `serde_json`'s default map is a `BTreeMap`, so this comes free and
//!   survives struct field reordering.
//! * **Floats quantized** to [`FLOAT_PRECISION`] decimal places, killing accumulated
//!   drift. Integral values print without a decimal point.
//! * **Arrays of primitives inlined.** A transform is six numbers on one line, not
//!   eight. This is for readability rather than determinism, and it meaningfully
//!   shrinks what a model has to read.
//! * **Trailing newline**, so the files behave in a terminal.
//!
//! Path geometry is exempt: it lives inside `d` strings already formatted by the
//! geometry kernel at its own precision.

use crate::error::{DocError, Result};
use serde::{ser, Serialize};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::fmt;

/// Decimal places retained for numbers in the document.
///
/// Deliberately looser than the geometry kernel's coordinate precision: this bound also
/// applies to easing control points and normalized keyframe times, where four places
/// would be visible.
pub const FLOAT_PRECISION: i32 = 6;

/// Above this magnitude, quantization would cost more precision than the float noise it
/// removes, so we leave the value alone.
const QUANTIZE_LIMIT: f64 = 1e12;

/// Serialize to the canonical JSON text for a document file.
pub fn to_canonical_string<T: Serialize>(value: &T) -> Result<String> {
    ensure_finite(value)?;
    let v = canonicalize(serde_json::to_value(value)?);
    let mut out = String::new();
    write_value(&mut out, &v, 0);
    out.push('\n');
    Ok(out)
}

/// Quantize floats throughout a value tree, leaving structure alone.
pub fn canonicalize(value: Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            // Integers are already exact; only floats need taming.
            Some(f) if n.as_i64().is_none() && n.as_u64().is_none() => Value::from(quantize(f)),
            _ => Value::Number(n),
        },
        Value::Array(a) => Value::Array(a.into_iter().map(canonicalize).collect()),
        Value::Object(o) => {
            Value::Object(o.into_iter().map(|(k, v)| (k, canonicalize(v))).collect())
        }
        other => other,
    }
}

fn quantize(v: f64) -> f64 {
    if !v.is_finite() || v.abs() >= QUANTIZE_LIMIT {
        return v;
    }
    let scale = 10f64.powi(FLOAT_PRECISION);
    let r = (v * scale).round() / scale;
    // Collapse negative zero, which would otherwise serialize as `-0.0`.
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

// ---------------------------------------------------------------------------
// The non-finite guard
// ---------------------------------------------------------------------------

/// Refuse NaN and infinity before anything reaches a file.
///
/// JSON cannot spell them, so `serde_json` writes `null` instead — and `null` will not
/// read back into an `f64`. Left to run, a NaN width means a save that reports success
/// and a project that never opens again, with nothing in the file to say what went
/// wrong. A refused save is recoverable; a corrupt one is not.
///
/// The error names the offending value by its dotted path, e.g. `pages.0.width`.
pub fn ensure_finite<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    let trail = RefCell::new(Vec::new());
    match value.serialize(FiniteCheck { trail: &trail }) {
        Ok(()) => Ok(()),
        Err(ScanError::NonFinite(path)) => Err(DocError::InvalidValue {
            path,
            reason: "not a finite number — NaN and infinity cannot be stored".into(),
        }),
        Err(ScanError::Other(reason)) => Err(DocError::InvalidValue {
            path: String::new(),
            reason,
        }),
    }
}

#[derive(Debug)]
enum ScanError {
    /// Dotted path to the offending number.
    NonFinite(String),
    Other(String),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::NonFinite(path) => write!(f, "non-finite number at '{path}'"),
            ScanError::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ScanError {}

impl ser::Error for ScanError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ScanError::Other(msg.to_string())
    }
}

type Scan = std::result::Result<(), ScanError>;

/// A serializer that looks at numbers and discards everything else.
///
/// It has to be a serializer rather than a walk over [`Value`], because by the time a
/// value has become a `Value` the non-finite numbers are already indistinguishable from
/// the nulls a document legitimately contains.
#[derive(Clone, Copy)]
struct FiniteCheck<'a> {
    trail: &'a RefCell<Vec<String>>,
}

impl<'a> FiniteCheck<'a> {
    fn number(self, v: f64) -> Scan {
        if v.is_finite() {
            Ok(())
        } else {
            Err(ScanError::NonFinite(self.trail.borrow().join(".")))
        }
    }

    fn nested<T: ?Sized + Serialize>(self, segment: String, value: &T) -> Scan {
        self.trail.borrow_mut().push(segment);
        let result = value.serialize(self);
        self.trail.borrow_mut().pop();
        result
    }

    fn compound(self) -> Compound<'a> {
        Compound {
            check: self,
            index: 0,
            key: String::new(),
        }
    }
}

macro_rules! ignored {
    ($($method:ident($ty:ty)),* $(,)?) => {
        $(fn $method(self, _v: $ty) -> Scan { Ok(()) })*
    };
}

impl<'a> ser::Serializer for FiniteCheck<'a> {
    type Ok = ();
    type Error = ScanError;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    ignored!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
    );

    fn serialize_f32(self, v: f32) -> Scan {
        self.number(v as f64)
    }

    fn serialize_f64(self, v: f64) -> Scan {
        self.number(v)
    }

    fn serialize_none(self) -> Scan {
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Scan {
        v.serialize(self)
    }

    fn serialize_unit(self) -> Scan {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Scan {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Scan {
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _name: &'static str, v: &T) -> Scan {
        v.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        v: &T,
    ) -> Scan {
        self.nested(variant.to_string(), v)
    }

    fn serialize_seq(self, _len: Option<usize>) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }

    fn serialize_tuple(self, _len: usize) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }

    fn serialize_map(self, _len: Option<usize>) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> std::result::Result<Compound<'a>, ScanError> {
        Ok(self.compound())
    }
}

/// Position within whatever container is currently being walked.
struct Compound<'a> {
    check: FiniteCheck<'a>,
    index: usize,
    key: String,
}

impl Compound<'_> {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Scan {
        let segment = self.index.to_string();
        self.index += 1;
        self.check.nested(segment, value)
    }
}

macro_rules! sequence_of {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for Compound<'_> {
            type Ok = ();
            type Error = ScanError;

            fn $method<T: ?Sized + Serialize>(&mut self, value: &T) -> Scan {
                self.element(value)
            }

            fn end(self) -> Scan {
                Ok(())
            }
        }
    };
}

sequence_of!(SerializeSeq, serialize_element);
sequence_of!(SerializeTuple, serialize_element);
sequence_of!(SerializeTupleStruct, serialize_field);
sequence_of!(SerializeTupleVariant, serialize_field);

macro_rules! fields_of {
    ($trait:ident) => {
        impl ser::$trait for Compound<'_> {
            type Ok = ();
            type Error = ScanError;

            fn serialize_field<T: ?Sized + Serialize>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> Scan {
                self.check.nested(key.to_string(), value)
            }

            fn end(self) -> Scan {
                Ok(())
            }
        }
    };
}

fields_of!(SerializeStruct);
fields_of!(SerializeStructVariant);

impl ser::SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = ScanError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Scan {
        // Every map in the document model is keyed by a string, and keeping the text is
        // what lets an error say `spacing.gutter` rather than `spacing.2`.
        self.key = match serde_json::to_value(key) {
            Ok(Value::String(s)) => s,
            Ok(other) => other.to_string(),
            Err(_) => String::new(),
        };
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Scan {
        let key = std::mem::take(&mut self.key);
        self.check.nested(key, value)
    }

    fn end(self) -> Scan {
        Ok(())
    }
}

/// Should this array be written on one line?
///
/// Only when it contains nothing nested — a list of numbers, strings or booleans.
/// Nesting means real structure, and real structure is easier to read expanded.
fn is_inlinable(a: &[Value]) -> bool {
    a.iter()
        .all(|v| !matches!(v, Value::Array(_) | Value::Object(_)))
}

fn write_value(out: &mut String, v: &Value, indent: usize) {
    match v {
        Value::Object(map) => write_object(out, map, indent),
        Value::Array(a) if a.is_empty() => out.push_str("[]"),
        Value::Array(a) if is_inlinable(a) => {
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_scalar(out, item);
            }
            out.push(']');
        }
        Value::Array(a) => {
            out.push_str("[\n");
            for (i, item) in a.iter().enumerate() {
                push_indent(out, indent + 1);
                write_value(out, item, indent + 1);
                if i + 1 < a.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push(']');
        }
        scalar => write_scalar(out, scalar),
    }
}

fn write_object(out: &mut String, map: &Map<String, Value>, indent: usize) {
    if map.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    let len = map.len();
    for (i, (k, val)) in map.iter().enumerate() {
        push_indent(out, indent + 1);
        out.push_str(&escape_string(k));
        out.push_str(": ");
        write_value(out, val, indent + 1);
        if i + 1 < len {
            out.push(',');
        }
        out.push('\n');
    }
    push_indent(out, indent);
    out.push('}');
}

fn write_scalar(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&format_number(n)),
        Value::String(s) => out.push_str(&escape_string(s)),
        // Only reachable if a nested value slips into an "inlinable" array; falling
        // back to compact JSON keeps the output valid rather than silently wrong.
        other => out.push_str(&other.to_string()),
    }
}

/// Print a number the way a person would write it.
///
/// A width of 800 is stored as an `f64` and would otherwise serialize as `800.0`. That
/// is correct and ugly, and the ugliness is multiplied across every coordinate in every
/// file. Dropping the empty fraction stays stable through a round trip: reading `800`
/// back into an `f64` field gives the same value, and re-serializing gives the same
/// text.
fn format_number(n: &serde_json::Number) -> String {
    if let Some(f) = n.as_f64() {
        if n.as_i64().is_none() && n.as_u64().is_none() && f.fract() == 0.0 && f.abs() < 1e15 {
            return format!("{}", f as i64);
        }
    }
    n.to_string()
}

fn escape_string(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn float_noise_is_removed() {
        let v = json!({ "x": 0.1 + 0.2 });
        assert_eq!(to_canonical_string(&v).unwrap(), "{\n  \"x\": 0.3\n}\n");
    }

    #[test]
    fn keys_come_out_sorted_regardless_of_input_order() {
        let a = to_canonical_string(&json!({ "z": 1, "a": 2, "m": 3 })).unwrap();
        let b = to_canonical_string(&json!({ "m": 3, "z": 1, "a": 2 })).unwrap();
        assert_eq!(a, b);
        assert!(a.find("\"a\"").unwrap() < a.find("\"m\"").unwrap());
    }

    #[test]
    fn primitive_arrays_stay_on_one_line() {
        let s = to_canonical_string(&json!({ "transform": [1, 0, 0, 1, 12.5, -4] })).unwrap();
        assert_eq!(s, "{\n  \"transform\": [1, 0, 0, 1, 12.5, -4]\n}\n");
    }

    #[test]
    fn arrays_of_objects_are_expanded() {
        let s = to_canonical_string(&json!({ "a": [{ "b": 1 }] })).unwrap();
        assert!(s.contains("\"a\": [\n"), "got {s}");
    }

    #[test]
    fn serializing_is_idempotent_through_a_reparse() {
        let doc = json!({
            "pages": [{ "name": "Home", "root": { "id": "nd_1", "opacity": 1.0 / 3.0 } }],
            "transform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            "roles": ["hero", "card"],
        });
        let once = to_canonical_string(&doc).unwrap();
        let reparsed: Value = serde_json::from_str(&once).unwrap();
        let twice = to_canonical_string(&reparsed).unwrap();
        assert_eq!(once, twice, "canonical form is not a fixed point");
    }

    #[test]
    fn whole_numbers_lose_their_empty_fraction() {
        let s = to_canonical_string(&json!({ "width": 800.0f64, "opacity": 0.5f64 })).unwrap();
        assert_eq!(s, "{\n  \"opacity\": 0.5,\n  \"width\": 800\n}\n");
    }

    #[test]
    fn dropping_the_fraction_survives_a_round_trip() {
        // The risk: `800.0` becomes `800`, reparses as an integer, and something
        // downstream serializes it differently the second time.
        let once = to_canonical_string(&json!({ "width": 800.0f64 })).unwrap();
        let reparsed: Value = serde_json::from_str(&once).unwrap();
        assert_eq!(to_canonical_string(&reparsed).unwrap(), once);

        #[derive(serde::Serialize, serde::Deserialize)]
        struct Sized {
            width: f64,
        }
        let typed: Sized = serde_json::from_str(&once).unwrap();
        assert_eq!(typed.width, 800.0);
        assert_eq!(to_canonical_string(&typed).unwrap(), once);
    }

    #[test]
    fn negative_zero_collapses() {
        let s = to_canonical_string(&json!({ "x": -0.0f64 })).unwrap();
        assert!(s.contains("\"x\": 0"), "got {s}");
    }

    #[test]
    fn very_large_numbers_are_left_intact() {
        let big = 1.234e20;
        let v = canonicalize(json!({ "x": big }));
        assert_eq!(v["x"].as_f64().unwrap(), big);
    }

    #[test]
    fn empty_containers_are_compact() {
        let s = to_canonical_string(&json!({ "a": {}, "b": [] })).unwrap();
        assert_eq!(s, "{\n  \"a\": {},\n  \"b\": []\n}\n");
    }

    #[test]
    fn strings_are_escaped() {
        let s = to_canonical_string(&json!({ "t": "line\nbreak \"quoted\"" })).unwrap();
        assert!(s.contains("\\n") && s.contains("\\\""), "got {s}");
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["t"], "line\nbreak \"quoted\"");
    }

    #[test]
    fn output_ends_with_a_newline() {
        assert!(to_canonical_string(&json!({})).unwrap().ends_with('\n'));
    }

    #[test]
    fn non_finite_numbers_are_refused_rather_than_written_as_null() {
        // `serde_json` spells NaN and infinity `null`, and `null` will not read back
        // into an `f64` — passing them through means a save that reports success and a
        // file nobody can open again.
        #[derive(serde::Serialize)]
        struct Rect {
            width: f64,
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = to_canonical_string(&Rect { width: bad })
                .unwrap_err()
                .to_string();
            assert!(err.contains("width"), "unhelpful error: {err}");
        }
        assert!(to_canonical_string(&Rect { width: 800.0 }).is_ok());
    }

    #[test]
    fn a_refused_number_is_named_by_its_path() {
        #[derive(serde::Serialize)]
        struct Page {
            nodes: Vec<Rect>,
        }
        #[derive(serde::Serialize)]
        struct Rect {
            width: f64,
        }
        let err = to_canonical_string(&Page {
            nodes: vec![Rect { width: 1.0 }, Rect { width: f64::NAN }],
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("nodes.1.width"), "got {err}");
    }

    #[test]
    fn map_keys_reach_the_path_too() {
        let spacing = std::collections::BTreeMap::from([("gutter".to_string(), f64::INFINITY)]);
        let err = to_canonical_string(&spacing).unwrap_err().to_string();
        assert!(err.contains("gutter"), "got {err}");
    }
}
