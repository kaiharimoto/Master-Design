//! Hit-testing and nearest-point queries.
//!
//! These run on every pointer move, which is exactly why the kernel is compiled to
//! WASM and called in-process rather than over Tauri IPC.

use crate::{parse, GeomError, DEFAULT_TOLERANCE};
use kurbo::{ParamCurve, ParamCurveNearest, Point, Shape};
use serde::{Deserialize, Serialize};

/// How to decide whether a point is "inside" a self-overlapping shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// Result of a nearest-point query — where on the path the hit landed, and how far.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearestPoint {
    pub x: f64,
    pub y: f64,
    pub distance: f64,
    /// Index of the segment the point landed on, counting from the start of the path.
    pub segment: usize,
    /// Parameter within that segment, 0..1.
    pub t: f64,
}

/// Is the point inside the filled region of the path?
pub fn hit_test_fill(d: &str, x: f64, y: f64, rule: FillRule) -> Result<bool, GeomError> {
    let path = parse(d)?;
    let winding = path.winding(Point::new(x, y));
    Ok(match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    })
}

/// Is the point within `width / 2 + tolerance` of the path's outline?
///
/// `tolerance` is where touch input gets its slop: a fingertip should be able to grab
/// a hairline stroke, so the studio passes a larger value on touch than on mouse.
pub fn hit_test_stroke(
    d: &str,
    x: f64,
    y: f64,
    width: f64,
    tolerance: f64,
) -> Result<bool, GeomError> {
    let reach = width.max(0.0) / 2.0 + tolerance.max(0.0);
    match nearest_point_on_path(d, x, y) {
        Ok(n) => Ok(n.distance <= reach),
        Err(GeomError::EmptyPath) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Closest point on the path to `(x, y)`.
///
/// Drives snapping, the "add anchor on path" tool, and the loupe's magnet behaviour
/// when a finger drags near an existing edge.
pub fn nearest_point_on_path(d: &str, x: f64, y: f64) -> Result<NearestPoint, GeomError> {
    let path = parse(d)?;
    let target = Point::new(x, y);

    let mut best: Option<NearestPoint> = None;
    for (i, seg) in path.segments().enumerate() {
        let n = seg.nearest(target, DEFAULT_TOLERANCE);
        let p = seg.eval(n.t);
        let distance = n.distance_sq.sqrt();
        if best.as_ref().is_none_or(|b| distance < b.distance) {
            best = Some(NearestPoint { x: p.x, y: p.y, distance, segment: i, t: n.t });
        }
    }

    best.ok_or(GeomError::EmptyPath)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = "M 0 0 L 10 0 L 10 10 L 0 10 Z";

    #[test]
    fn inside_and_outside_a_square() {
        assert!(hit_test_fill(SQUARE, 5.0, 5.0, FillRule::NonZero).unwrap());
        assert!(!hit_test_fill(SQUARE, 15.0, 5.0, FillRule::NonZero).unwrap());
    }

    #[test]
    fn even_odd_sees_a_hole_where_nonzero_does_not() {
        // Outer ring clockwise, inner ring also clockwise: same winding direction, so
        // non-zero fills straight through while even-odd punches a hole.
        let d = "M 0 0 L 30 0 L 30 30 L 0 30 Z M 10 10 L 20 10 L 20 20 L 10 20 Z";
        assert!(hit_test_fill(d, 15.0, 15.0, FillRule::NonZero).unwrap());
        assert!(!hit_test_fill(d, 15.0, 15.0, FillRule::EvenOdd).unwrap());
    }

    #[test]
    fn stroke_hit_respects_width_and_slop() {
        // 3 units away from the left edge.
        assert!(!hit_test_stroke(SQUARE, -3.0, 5.0, 2.0, 0.5).unwrap());
        assert!(hit_test_stroke(SQUARE, -3.0, 5.0, 2.0, 2.5).unwrap());
    }

    #[test]
    fn nearest_point_lands_on_the_edge() {
        let n = nearest_point_on_path(SQUARE, -4.0, 5.0).unwrap();
        assert!((n.x - 0.0).abs() < 1e-6, "got {n:?}");
        assert!((n.distance - 4.0).abs() < 1e-6, "got {n:?}");
    }
}
