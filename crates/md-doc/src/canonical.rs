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

use crate::error::Result;
use serde::Serialize;
use serde_json::{Map, Value};

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
}
