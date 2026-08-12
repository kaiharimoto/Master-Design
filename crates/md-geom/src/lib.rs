//! # md-geom
//!
//! The geometry kernel behind Master Design's vector editing. It is deliberately
//! self-contained: it knows about paths, not about documents. That keeps it cheap to
//! compile to WebAssembly and run *inside* the webview, which matters because
//! hit-testing happens on every pointer move and cannot afford an IPC round-trip.
//!
//! Paths cross every boundary in this system as **SVG path data strings**. That choice
//! buys a lot: the format is already understood by the browser, by every design tool,
//! and by the AI models that edit our documents; it diffs as a single line; and export
//! is a no-op. Internally we parse to [`kurbo::BezPath`], do the work, and serialize
//! back out in a normalized form (absolute `M`/`C`/`Z` only).

mod boolean;
mod error;
mod fit;
mod hit;
mod path;
mod shapes;
mod stroke;

pub use boolean::{boolean_op, BoolOp};
pub use error::GeomError;
pub use fit::{fit_cubics, simplify_rdp};
pub use hit::{hit_test_fill, hit_test_stroke, nearest_point_on_path, FillRule, NearestPoint};
pub use path::{
    bounds, flatten_path, normalize, parse, path_length, point_at_length, to_svg, transform_path,
    Bounds,
};
pub use shapes::{ellipse_path, expand_radii, polygon_path, rect_path, round_corners, star_path};
pub use stroke::{outline_stroke, LineCap, LineJoin, StrokeStyle};

/// Tolerance used whenever we approximate curves by line segments.
///
/// One hundredth of a document unit sits comfortably below a device pixel at any zoom
/// a human will use, while keeping flattened point counts small enough that boolean
/// operations stay fast on mobile.
pub const DEFAULT_TOLERANCE: f64 = 0.01;

/// Decimal places retained when serializing coordinates.
///
/// Must match `md-doc`'s canonical serializer, otherwise a round-trip through the
/// geometry kernel would produce spurious diffs.
pub const COORD_PRECISION: usize = 4;

/// Is `v` safe to write out as a coordinate?
///
/// [`fmt_coord`] cannot report a bad value — it is infallible by design — so anything
/// that can produce a non-finite number (a singular matrix, a divide by zero in a layout
/// solve, a malformed import) should ask here and raise its own error while it still has
/// the context to say what went wrong.
pub fn is_finite_coord(v: f64) -> bool {
    v.is_finite()
}

/// Format a float the way every coordinate in this system is formatted: rounded to
/// [`COORD_PRECISION`] and stripped of trailing zeros, so `1.0` prints as `1` and
/// `0.30000000000000004` prints as `0.3`.
///
/// The stripping is what makes it idempotent — formatting an already-formatted value
/// reproduces it byte for byte, which is the property canonical serialization relies on.
///
/// NaN and both infinities are written as `0`. That is a deliberate last line of defence
/// rather than error handling: this runs on every coordinate of every serialization and
/// every WASM call, so it stays total, and `NaN` in path data would produce a document no
/// SVG parser — ours included — could read back. The cost is real, though, and it is why
/// [`is_finite_coord`] exists: by the time a value reaches here the corruption is
/// indistinguishable from a legitimate origin, so callers that can generate non-finite
/// coordinates must check before formatting instead of relying on this.
pub fn fmt_coord(v: f64) -> String {
    if !is_finite_coord(v) {
        return "0".to_string();
    }
    let mut s = format!("{:.*}", COORD_PRECISION, v);
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" {
        s = "0".to_string();
    }
    s
}

#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coord_formatting_is_idempotent() {
        for raw in [
            0.0,
            -0.0,
            1.0,
            0.30000000000000004,
            -12.345678,
            1e-9,
            99999.99999,
        ] {
            let once = fmt_coord(raw);
            let twice = fmt_coord(once.parse::<f64>().unwrap());
            assert_eq!(once, twice, "not idempotent for {raw}");
        }
    }

    #[test]
    fn coord_formatting_is_readable() {
        assert_eq!(fmt_coord(1.0), "1");
        assert_eq!(fmt_coord(-0.0), "0");
        assert_eq!(fmt_coord(0.30000000000000004), "0.3");
        assert_eq!(fmt_coord(2.5), "2.5");
    }

    #[test]
    fn non_finite_coords_are_written_as_zero() {
        // Pinning the documented fallback: the output has to stay parseable, so a
        // coordinate that has already gone wrong lands at the origin rather than
        // spelling `NaN` into path data.
        assert_eq!(fmt_coord(f64::NAN), "0");
        assert_eq!(fmt_coord(f64::INFINITY), "0");
        assert_eq!(fmt_coord(f64::NEG_INFINITY), "0");
    }

    #[test]
    fn non_finite_coords_are_detectable_before_formatting() {
        assert!(!is_finite_coord(f64::NAN));
        assert!(!is_finite_coord(f64::INFINITY));
        assert!(!is_finite_coord(f64::NEG_INFINITY));
        assert!(is_finite_coord(0.0));
        assert!(is_finite_coord(-12.345678));
        assert!(is_finite_coord(f64::MAX));
    }

    #[test]
    fn a_non_finite_coordinate_is_indistinguishable_from_the_origin() {
        // Why the check above has to exist: once formatted, the corruption is gone.
        assert_eq!(fmt_coord(f64::NAN), fmt_coord(0.0));
    }
}
