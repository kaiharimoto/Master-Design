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
pub use shapes::{ellipse_path, polygon_path, rect_path, round_corners, star_path};
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

/// Format a float the way every coordinate in this system is formatted: rounded to
/// [`COORD_PRECISION`] and stripped of trailing zeros, so `1.0` prints as `1` and
/// `0.30000000000000004` prints as `0.3`.
///
/// The stripping is what makes it idempotent — formatting an already-formatted value
/// reproduces it byte for byte, which is the property canonical serialization relies on.
pub fn fmt_coord(v: f64) -> String {
    if !v.is_finite() {
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
}
