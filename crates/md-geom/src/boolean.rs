//! Boolean operations on paths.
//!
//! ## How this works, and what it costs
//!
//! Exact bezier-on-bezier clipping is a research problem, not a milestone. Every
//! shipping vector tool takes the same route we take here: flatten to polygons within
//! a tolerance, run a robust polygon clipper, then fit curves back onto the result.
//!
//! The consequence worth knowing: output geometry is an *approximation* of the ideal
//! curve intersection, accurate to `tolerance`. At the default of one hundredth of a
//! unit that is well under a device pixel at any sane zoom. The refit step is what
//! keeps the result editable — without it, subtracting one circle from another would
//! hand the user a path with several hundred anchor points.

use crate::fit::fit_cubics;
use crate::path::{flatten_bez, to_svg};
use crate::{parse, GeomError, DEFAULT_TOLERANCE};
use geo::algorithm::bool_ops::BooleanOps;
use geo::{Coord, LineString, MultiPolygon, Polygon};
use kurbo::BezPath;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BoolOp {
    /// Everything covered by any operand.
    Union,
    /// The first operand with every later operand removed.
    Subtract,
    /// Only what every operand covers.
    Intersect,
    /// Covered by an odd number of operands.
    Exclude,
}

/// Combine two or more paths.
///
/// `refit` trades exactness for editability: on, the result is fitted back to cubic
/// beziers with a handful of anchors; off, it stays as dense polylines. The editor
/// leaves it on — a designer who subtracts two shapes wants a shape they can still
/// pull handles on.
pub fn boolean_op(
    paths: &[String],
    op: BoolOp,
    tolerance: f64,
    refit: bool,
) -> Result<String, GeomError> {
    if paths.len() < 2 {
        return Err(GeomError::InvalidParam(
            "a boolean operation needs at least two paths".into(),
        ));
    }
    let tolerance = if tolerance > 0.0 {
        tolerance
    } else {
        DEFAULT_TOLERANCE
    };

    let mut operands = Vec::with_capacity(paths.len());
    for d in paths {
        operands.push(path_to_multipolygon(d, tolerance)?);
    }

    let mut acc = operands.remove(0);
    for next in operands {
        acc = match op {
            BoolOp::Union => acc.union(&next),
            BoolOp::Subtract => acc.difference(&next),
            BoolOp::Intersect => acc.intersection(&next),
            BoolOp::Exclude => acc.xor(&next),
        };
    }

    Ok(multipolygon_to_path(&acc, tolerance, refit))
}

fn path_to_multipolygon(d: &str, tolerance: f64) -> Result<MultiPolygon<f64>, GeomError> {
    let path = parse(d)?;
    let rings = flatten_bez(&path, tolerance);
    Ok(rings_to_multipolygon(rings))
}

/// Assemble flattened rings into polygons, deciding which rings are holes by nesting
/// depth: a ring contained in an odd number of other rings is a hole.
///
/// This is even-odd nesting. It is the interpretation a designer expects from a
/// compound path — draw a small circle inside a big one and you get a donut,
/// regardless of which direction each was drawn in.
fn rings_to_multipolygon(rings: Vec<Vec<[f64; 2]>>) -> MultiPolygon<f64> {
    let rings: Vec<Vec<[f64; 2]>> = rings.into_iter().filter(|r| r.len() >= 4).collect();
    if rings.is_empty() {
        return MultiPolygon::new(Vec::new());
    }

    let depths: Vec<usize> = rings
        .iter()
        .enumerate()
        .map(|(i, ring)| {
            let probe = interior_probe(ring);
            rings
                .iter()
                .enumerate()
                .filter(|(j, other)| *j != i && point_in_ring(probe, other))
                .count()
        })
        .collect();

    // Exteriors are at even depth. Each hole attaches to the smallest exterior that
    // contains it, which for well-formed art is its immediate parent.
    let mut polygons: Vec<(usize, Polygon<f64>)> = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        if depths[i] % 2 == 0 {
            polygons.push((i, Polygon::new(to_linestring(ring), Vec::new())));
        }
    }

    for (i, ring) in rings.iter().enumerate() {
        if depths[i] % 2 == 0 {
            continue;
        }
        let probe = interior_probe(ring);
        let parent = polygons
            .iter_mut()
            .filter(|(_, poly)| point_in_ring(probe, &ring_coords(poly.exterior())))
            .min_by(|(_, a), (_, b)| {
                ring_area(&ring_coords(a.exterior()))
                    .abs()
                    .partial_cmp(&ring_area(&ring_coords(b.exterior())).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((_, poly)) = parent {
            poly.interiors_push(to_linestring(ring));
        }
    }

    MultiPolygon::new(polygons.into_iter().map(|(_, p)| p).collect())
}

fn to_linestring(ring: &[[f64; 2]]) -> LineString<f64> {
    let mut coords: Vec<Coord<f64>> = ring.iter().map(|p| Coord { x: p[0], y: p[1] }).collect();
    if coords.first() != coords.last() {
        if let Some(first) = coords.first().copied() {
            coords.push(first);
        }
    }
    LineString::new(coords)
}

fn ring_coords(ls: &LineString<f64>) -> Vec<[f64; 2]> {
    ls.0.iter().map(|c| [c.x, c.y]).collect()
}

fn ring_area(ring: &[[f64; 2]]) -> f64 {
    let mut sum = 0.0;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        sum += a[0] * b[1] - b[0] * a[1];
    }
    sum / 2.0
}

/// A point that is inside the ring rather than on its boundary.
///
/// Using a vertex would be ambiguous for containment tests, so we take the centroid of
/// the first triangle and fall back to the vertex average if that lands on an edge.
fn interior_probe(ring: &[[f64; 2]]) -> [f64; 2] {
    if ring.len() >= 3 {
        let c = [
            (ring[0][0] + ring[1][0] + ring[2][0]) / 3.0,
            (ring[0][1] + ring[1][1] + ring[2][1]) / 3.0,
        ];
        if point_in_ring(c, ring) {
            return c;
        }
    }
    let n = ring.len().max(1) as f64;
    let sx: f64 = ring.iter().map(|p| p[0]).sum();
    let sy: f64 = ring.iter().map(|p| p[1]).sum();
    [sx / n, sy / n]
}

/// Ray-casting containment test.
fn point_in_ring(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (ring[i][0], ring[i][1]);
        let (xj, yj) = (ring[j][0], ring[j][1]);
        if (yi > p[1]) != (yj > p[1]) {
            let x_cross = (xj - xi) * (p[1] - yi) / (yj - yi) + xi;
            if p[0] < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn multipolygon_to_path(mp: &MultiPolygon<f64>, tolerance: f64, refit: bool) -> String {
    let mut out = BezPath::new();
    for poly in mp.iter() {
        append_ring(&mut out, &ring_coords(poly.exterior()), tolerance, refit);
        for hole in poly.interiors() {
            append_ring(&mut out, &ring_coords(hole), tolerance, refit);
        }
    }
    to_svg(&out)
}

fn append_ring(out: &mut BezPath, ring: &[[f64; 2]], tolerance: f64, refit: bool) {
    // Drop the duplicated closing vertex; `close_path` puts it back.
    let mut pts = ring.to_vec();
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    if pts.len() < 3 {
        return;
    }

    if refit {
        // Fit around the loop, then close. Fitting error is allowed to be a few times
        // the flattening tolerance: the polyline is already an approximation, and a
        // looser fit is what buys the anchor-count reduction that makes it editable.
        //
        // The pre-pass tolerance stays well under the fit tolerance on purpose.
        // Simplifying aggressively first would throw away the very samples the fitter
        // uses to measure its own error, and it would report a good fit for a curve that
        // wanders far from the shape between the points it kept.
        let mut looped = pts.clone();
        looped.push(pts[0]);
        let simplified = crate::fit::simplify_rdp(&looped, tolerance * 0.25);
        let fitted = fit_cubics(&simplified, tolerance * 4.0);
        if fitted.elements().len() > 1 {
            for el in fitted.elements() {
                out.push(*el);
            }
            out.close_path();
            return;
        }
    }

    out.move_to((pts[0][0], pts[0][1]));
    for p in &pts[1..] {
        out.line_to((p[0], p[1]));
    }
    out.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bounds, hit_test_fill, FillRule};

    fn square(x: f64, y: f64, s: f64) -> String {
        format!(
            "M {x} {y} L {} {y} L {} {} L {x} {} Z",
            x + s,
            x + s,
            y + s,
            y + s
        )
    }

    #[test]
    fn union_of_two_overlapping_squares_spans_both() {
        let out = boolean_op(
            &[square(0.0, 0.0, 10.0), square(5.0, 0.0, 10.0)],
            BoolOp::Union,
            0.01,
            false,
        )
        .unwrap();
        let b = bounds(&out).unwrap();
        assert!((b.w - 15.0).abs() < 0.2, "got {b:?}");
        assert!((b.h - 10.0).abs() < 0.2, "got {b:?}");
    }

    #[test]
    fn subtract_removes_the_overlap() {
        let out = boolean_op(
            &[square(0.0, 0.0, 10.0), square(5.0, -5.0, 10.0)],
            BoolOp::Subtract,
            0.01,
            false,
        )
        .unwrap();
        // A point in the removed region is now outside; one in the kept region is inside.
        assert!(!hit_test_fill(&out, 7.0, 2.0, FillRule::NonZero).unwrap());
        assert!(hit_test_fill(&out, 2.0, 5.0, FillRule::NonZero).unwrap());
    }

    #[test]
    fn intersect_keeps_only_the_overlap() {
        let out = boolean_op(
            &[square(0.0, 0.0, 10.0), square(5.0, 5.0, 10.0)],
            BoolOp::Intersect,
            0.01,
            false,
        )
        .unwrap();
        let b = bounds(&out).unwrap();
        assert!((b.w - 5.0).abs() < 0.2, "got {b:?}");
        assert!((b.h - 5.0).abs() < 0.2, "got {b:?}");
    }

    #[test]
    fn a_hole_survives_the_round_trip() {
        // Big square with a small square inside it: even-odd nesting should make a donut.
        let donut = format!("{} {}", square(0.0, 0.0, 30.0), square(10.0, 10.0, 10.0));
        let out = boolean_op(
            &[donut, square(-100.0, -100.0, 1.0)],
            BoolOp::Union,
            0.01,
            false,
        )
        .unwrap();
        assert!(
            !hit_test_fill(&out, 15.0, 15.0, FillRule::EvenOdd).unwrap(),
            "hole was filled in"
        );
        assert!(hit_test_fill(&out, 2.0, 2.0, FillRule::EvenOdd).unwrap());
    }

    #[test]
    fn refit_compresses_a_circle_union() {
        let circle = |cx: f64| crate::ellipse_path(cx, 0.0, 50.0, 50.0);
        let dense = boolean_op(&[circle(0.0), circle(40.0)], BoolOp::Union, 0.01, false).unwrap();
        let fitted = boolean_op(&[circle(0.0), circle(40.0)], BoolOp::Union, 0.01, true).unwrap();
        let dense_segs = parse(&dense).unwrap().segments().count();
        let fitted_segs = parse(&fitted).unwrap().segments().count();
        assert!(
            fitted_segs * 4 < dense_segs,
            "refit should collapse anchors: {fitted_segs} vs {dense_segs}"
        );
    }

    #[test]
    fn one_operand_is_an_error() {
        assert!(boolean_op(&[square(0.0, 0.0, 1.0)], BoolOp::Union, 0.01, false).is_err());
    }
}
