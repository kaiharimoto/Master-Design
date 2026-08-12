//! Primitive shape generators and live corner rounding.
//!
//! The document keeps rectangles and ellipses as *live* shapes with their own
//! parameters rather than baking them to paths, so a designer can still change a
//! corner radius after the fact. These functions are how a live shape becomes geometry
//! at render, export and hit-test time.

use crate::path::to_svg;
use crate::{parse, GeomError};
use kurbo::{BezPath, Ellipse, PathEl, Point, RoundedRect, RoundedRectRadii, Shape, Vec2};

const ARC_TOLERANCE: f64 = 0.01;

/// Rectangle with independent corner radii, ordered top-left, top-right,
/// bottom-right, bottom-left — the same order CSS uses.
pub fn rect_path(x: f64, y: f64, w: f64, h: f64, radii: [f64; 4]) -> String {
    if w <= 0.0 || h <= 0.0 {
        return String::new();
    }

    // A radius can never exceed half the shorter side, or opposite corners would
    // overlap and the path would fold in on itself.
    let cap = (w.min(h)) / 2.0;
    let r = RoundedRectRadii::new(
        radii[0].clamp(0.0, cap),
        radii[1].clamp(0.0, cap),
        radii[2].clamp(0.0, cap),
        radii[3].clamp(0.0, cap),
    );

    if radii.iter().all(|v| *v <= 0.0) {
        let mut p = BezPath::new();
        p.move_to((x, y));
        p.line_to((x + w, y));
        p.line_to((x + w, y + h));
        p.line_to((x, y + h));
        p.close_path();
        return to_svg(&p);
    }

    let rect = RoundedRect::new(x, y, x + w, y + h, r);
    to_svg(&rect.to_path(ARC_TOLERANCE))
}

/// Ellipse centred on `(cx, cy)`.
pub fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64) -> String {
    if rx <= 0.0 || ry <= 0.0 {
        return String::new();
    }
    let e = Ellipse::new(Point::new(cx, cy), Vec2::new(rx, ry), 0.0);
    to_svg(&e.to_path(ARC_TOLERANCE))
}

/// Regular polygon with `sides` vertices, first vertex placed at `rotation` radians
/// measured from twelve o'clock.
pub fn polygon_path(cx: f64, cy: f64, radius: f64, sides: u32, rotation: f64) -> String {
    if sides < 3 || radius <= 0.0 {
        return String::new();
    }
    let mut p = BezPath::new();
    for i in 0..sides {
        let a = rotation - std::f64::consts::FRAC_PI_2
            + (i as f64) * std::f64::consts::TAU / (sides as f64);
        let pt = (cx + radius * a.cos(), cy + radius * a.sin());
        if i == 0 {
            p.move_to(pt);
        } else {
            p.line_to(pt);
        }
    }
    p.close_path();
    to_svg(&p)
}

/// Star alternating between an outer and inner radius.
pub fn star_path(
    cx: f64,
    cy: f64,
    outer: f64,
    inner: f64,
    points: u32,
    rotation: f64,
) -> String {
    if points < 3 || outer <= 0.0 || inner <= 0.0 {
        return String::new();
    }
    let mut p = BezPath::new();
    let step = std::f64::consts::PI / (points as f64);
    for i in 0..(points * 2) {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = rotation - std::f64::consts::FRAC_PI_2 + (i as f64) * step;
        let pt = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            p.move_to(pt);
        } else {
            p.line_to(pt);
        }
    }
    p.close_path();
    to_svg(&p)
}

/// Round the corners of a straight-edged path — Illustrator's live corners, applied to
/// pen-drawn polygons and to the shape primitives above.
///
/// Subpaths that already contain curves are passed through untouched: rounding a
/// curve-to-curve join needs an offset-curve solve, which belongs with the rest of the
/// advanced path work rather than here.
pub fn round_corners(d: &str, radius: f64) -> Result<String, GeomError> {
    if radius <= 0.0 {
        return Ok(d.to_string());
    }
    let path = parse(d)?;
    let mut out = BezPath::new();

    for sub in split_subpaths(&path) {
        if sub.has_curves || sub.points.len() < 3 {
            for el in &sub.elements {
                out.push(*el);
            }
            continue;
        }
        round_polyline(&mut out, &sub.points, sub.closed, radius);
    }

    Ok(to_svg(&out))
}

struct SubPath {
    elements: Vec<PathEl>,
    points: Vec<Point>,
    closed: bool,
    has_curves: bool,
}

fn split_subpaths(path: &BezPath) -> Vec<SubPath> {
    let mut subs: Vec<SubPath> = Vec::new();
    let mut cur: Option<SubPath> = None;

    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if let Some(s) = cur.take() {
                    subs.push(s);
                }
                cur = Some(SubPath {
                    elements: vec![*el],
                    points: vec![p],
                    closed: false,
                    has_curves: false,
                });
            }
            PathEl::LineTo(p) => {
                if let Some(s) = cur.as_mut() {
                    s.elements.push(*el);
                    s.points.push(p);
                }
            }
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => {
                if let Some(s) = cur.as_mut() {
                    s.elements.push(*el);
                    s.has_curves = true;
                }
            }
            PathEl::ClosePath => {
                if let Some(s) = cur.as_mut() {
                    s.elements.push(*el);
                    s.closed = true;
                }
                if let Some(s) = cur.take() {
                    subs.push(s);
                }
            }
        }
    }
    if let Some(s) = cur.take() {
        subs.push(s);
    }
    subs
}

fn round_polyline(out: &mut BezPath, points: &[Point], closed: bool, radius: f64) {
    // A closed ring whose last point repeats the first would produce a zero-length edge.
    let mut pts = points.to_vec();
    if closed && pts.len() > 1 && (pts[0] - *pts.last().unwrap()).hypot() < 1e-9 {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        for (i, p) in pts.iter().enumerate() {
            if i == 0 {
                out.move_to(*p);
            } else {
                out.line_to(*p);
            }
        }
        if closed {
            out.close_path();
        }
        return;
    }

    let range: Vec<usize> = if closed { (0..n).collect() } else { (1..n - 1).collect() };
    let mut started = false;

    if !closed {
        out.move_to(pts[0]);
        started = true;
    }

    for &i in &range {
        let prev = pts[(i + n - 1) % n];
        let corner = pts[i];
        let next = pts[(i + 1) % n];

        let v_in = corner - prev;
        let v_out = next - corner;
        let len_in = v_in.hypot();
        let len_out = v_out.hypot();
        if len_in < 1e-9 || len_out < 1e-9 {
            continue;
        }
        let u_in = v_in / len_in;
        let u_out = v_out / len_out;

        // Angle the path turns through at this corner.
        let cos_turn = (-u_in).dot(u_out).clamp(-1.0, 1.0);
        let interior = cos_turn.acos();
        let sweep = std::f64::consts::PI - interior;

        // Collinear or doubled-back corners have nothing to round.
        if sweep.abs() < 1e-6 || (std::f64::consts::PI - sweep).abs() < 1e-6 {
            if !started {
                out.move_to(corner);
                started = true;
            } else {
                out.line_to(corner);
            }
            continue;
        }

        // Never eat more than half of either adjacent edge.
        let trim = radius.min(len_in / 2.0).min(len_out / 2.0);
        let a = corner - u_in * trim;
        let b = corner + u_out * trim;

        // Circular arc through a and b, as a cubic. The 4/3·tan(θ/4) factor is the
        // standard single-segment arc approximation.
        let arc_radius = trim * (interior / 2.0).tan();
        let handle = (4.0 / 3.0) * (sweep / 4.0).tan() * arc_radius;

        if !started {
            out.move_to(a);
            started = true;
        } else {
            out.line_to(a);
        }
        out.curve_to(a + u_in * handle, b - u_out * handle, b);
    }

    if !closed {
        out.line_to(pts[n - 1]);
    } else {
        out.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bounds, hit_test_fill, FillRule};

    #[test]
    fn sharp_rect_is_four_straight_edges() {
        let d = rect_path(0.0, 0.0, 10.0, 20.0, [0.0; 4]);
        assert_eq!(d, "M 0 0 L 10 0 L 10 20 L 0 20 Z");
    }

    #[test]
    fn rounded_rect_keeps_its_bounds() {
        let d = rect_path(0.0, 0.0, 40.0, 20.0, [5.0; 4]);
        let b = bounds(&d).unwrap();
        assert!((b.w - 40.0).abs() < 0.05 && (b.h - 20.0).abs() < 0.05, "got {b:?}");
        // The corner is cut away.
        assert!(!hit_test_fill(&d, 0.3, 0.3, FillRule::NonZero).unwrap());
        assert!(hit_test_fill(&d, 20.0, 10.0, FillRule::NonZero).unwrap());
    }

    #[test]
    fn corner_radius_is_capped_at_half_the_short_side() {
        // Asking for 100 on a 20-tall rect should clamp to 10 and stay a valid shape.
        let d = rect_path(0.0, 0.0, 40.0, 20.0, [100.0; 4]);
        let b = bounds(&d).unwrap();
        assert!((b.h - 20.0).abs() < 0.05, "got {b:?}");
    }

    #[test]
    fn ellipse_has_the_right_extent() {
        let d = ellipse_path(0.0, 0.0, 30.0, 10.0);
        let b = bounds(&d).unwrap();
        assert!((b.w - 60.0).abs() < 0.05 && (b.h - 20.0).abs() < 0.05, "got {b:?}");
    }

    #[test]
    fn polygon_has_one_vertex_per_side() {
        let d = polygon_path(0.0, 0.0, 10.0, 6, 0.0);
        assert_eq!(d.matches('L').count(), 5);
    }

    #[test]
    fn star_alternates_radii() {
        let d = star_path(0.0, 0.0, 10.0, 4.0, 5, 0.0);
        assert_eq!(d.matches('L').count(), 9);
    }

    #[test]
    fn rounding_a_square_cuts_its_corners() {
        let square = "M 0 0 L 20 0 L 20 20 L 0 20 Z";
        let rounded = round_corners(square, 5.0).unwrap();
        assert!(rounded.contains('C'), "expected arcs, got {rounded}");
        assert!(!hit_test_fill(&rounded, 0.2, 0.2, FillRule::NonZero).unwrap());
        assert!(hit_test_fill(&rounded, 10.0, 10.0, FillRule::NonZero).unwrap());
        let b = bounds(&rounded).unwrap();
        assert!((b.w - 20.0).abs() < 0.05 && (b.h - 20.0).abs() < 0.05, "got {b:?}");
    }

    #[test]
    fn rounding_leaves_curved_subpaths_alone() {
        let curvy = "M 0 0 C 5 10 15 10 20 0 Z";
        assert_eq!(round_corners(curvy, 5.0).unwrap(), crate::normalize(curvy).unwrap());
    }

    #[test]
    fn zero_radius_is_a_no_op() {
        let square = "M 0 0 L 20 0 L 20 20 Z";
        assert_eq!(round_corners(square, 0.0).unwrap(), square);
    }
}
