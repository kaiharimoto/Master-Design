//! Turning point runs back into curves.
//!
//! Two operations that look similar but serve different masters:
//!
//! * [`simplify_rdp`] throws away points that do not change the shape. It is what keeps
//!   a freehand touch stroke from becoming a thousand anchors.
//! * [`fit_cubics`] is Schneider's least-squares curve fitting, which is how a polyline
//!   becomes an *editable* path with a handful of anchors and real bezier handles.
//!   Boolean operations flatten their inputs, so without this every subtract would
//!   leave the user with unmanageable geometry.

use kurbo::{BezPath, Point, Vec2};

/// Ramer–Douglas–Peucker: drop points that sit within `tolerance` of the line between
/// their surviving neighbours.
pub fn simplify_rdp(points: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    if points.len() < 3 || tolerance <= 0.0 {
        return points.to_vec();
    }

    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp_recurse(points, 0, points.len() - 1, tolerance, &mut keep);

    points
        .iter()
        .zip(keep.iter())
        .filter_map(|(p, k)| if *k { Some(*p) } else { None })
        .collect()
}

fn rdp_recurse(points: &[[f64; 2]], first: usize, last: usize, tol: f64, keep: &mut [bool]) {
    if last <= first + 1 {
        return;
    }

    let a = Point::new(points[first][0], points[first][1]);
    let b = Point::new(points[last][0], points[last][1]);

    let mut worst = 0.0;
    let mut worst_idx = first;
    for (i, p) in points.iter().enumerate().take(last).skip(first + 1) {
        let d = distance_to_segment(Point::new(p[0], p[1]), a, b);
        if d > worst {
            worst = d;
            worst_idx = i;
        }
    }

    if worst > tol {
        keep[worst_idx] = true;
        rdp_recurse(points, first, worst_idx, tol, keep);
        rdp_recurse(points, worst_idx, last, tol, keep);
    }
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len_sq = ab.hypot2();
    if len_sq < f64::EPSILON {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

/// Turn angle above which a vertex is treated as a hard corner rather than a sample of
/// a smooth curve.
///
/// Fitting *across* a corner is the failure that motivates this constant. Least-squares
/// fitting only measures error at the sample points, so a cubic asked to pass through
/// the four corners of a square hits all four and bulges enormously in between —
/// producing a shape whose bounding box is visibly larger than the square it came from,
/// with no error metric noticing. Splitting at corners first makes each span genuinely
/// smooth, which is the assumption the fitter is built on.
const CORNER_THRESHOLD: f64 = std::f64::consts::PI / 6.0; // 30°

/// Fit a sequence of cubic beziers through `points`, staying within `max_error`.
///
/// Runs of points that turn sharply are split at the corner and fitted independently, so
/// hard edges stay hard. Straight spans come back as line segments rather than
/// degenerate curves.
///
/// Returns an open path (no trailing `Z`) starting at the first point. Callers that
/// know their run is a closed ring should close the result themselves.
pub fn fit_cubics(points: &[[f64; 2]], max_error: f64) -> BezPath {
    let pts = dedupe(points);
    let mut out = BezPath::new();
    if pts.len() < 2 {
        return out;
    }

    out.move_to(pts[0]);
    let error = max_error.max(1e-9);

    for span in split_at_corners(&pts) {
        fit_span(span, error, &mut out);
    }
    out
}

/// Break a point run at hard corners. Spans overlap by one point, so consecutive spans
/// share the corner vertex and the output stays connected.
fn split_at_corners(pts: &[Point]) -> Vec<&[Point]> {
    let mut spans = Vec::new();
    let mut start = 0;

    for i in 1..pts.len().saturating_sub(1) {
        let incoming = unit(pts[i] - pts[i - 1]);
        let outgoing = unit(pts[i + 1] - pts[i]);
        if incoming.hypot() < 0.5 || outgoing.hypot() < 0.5 {
            continue;
        }
        let turn = incoming.dot(outgoing).clamp(-1.0, 1.0).acos();
        if turn > CORNER_THRESHOLD {
            spans.push(&pts[start..=i]);
            start = i;
        }
    }

    spans.push(&pts[start..]);
    spans
}

/// Fit one smooth span, assuming `out` already ends at `span[0]`.
fn fit_span(span: &[Point], error: f64, out: &mut BezPath) {
    match span.len() {
        0 | 1 => {}
        // A two-point span is a straight edge. Emitting it as a line rather than a
        // collinear cubic keeps boolean output readable and halves its size.
        2 => out.line_to(span[1]),
        _ => {
            let left = unit(span[1] - span[0]);
            let right = unit(span[span.len() - 2] - span[span.len() - 1]);
            fit_cubic(span, left, right, error, 0, out);
        }
    }
}

fn dedupe(points: &[[f64; 2]]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        let pt = Point::new(p[0], p[1]);
        if out.last().is_none_or(|last| (pt - *last).hypot() > 1e-9) {
            out.push(pt);
        }
    }
    out
}

fn unit(v: Vec2) -> Vec2 {
    let h = v.hypot();
    if h < f64::EPSILON {
        Vec2::new(0.0, 0.0)
    } else {
        v / h
    }
}

/// Recursion cap. Depth 24 already allows over sixteen million segments; anything
/// deeper is pathological input, and we would rather emit a slightly loose fit than
/// blow the stack on a device with a small thread stack.
const MAX_DEPTH: usize = 24;

/// How far outside tolerance a fit can be and still be worth reparameterizing rather
/// than splitting, as a multiple of the tolerance.
///
/// Generous on purpose. Reparameterizing is cheap and bounded by [`MAX_ITERATIONS`];
/// splitting is what costs the user anchor points, and anchor points are what they have
/// to drag around afterwards.
const ITERATION_BAND: f64 = 64.0;

/// Newton-Raphson passes per attempt. Schneider uses four; past that the parameter
/// values have stopped moving and we are only burning cycles.
const MAX_ITERATIONS: usize = 8;

fn fit_cubic(pts: &[Point], left: Vec2, right: Vec2, error: f64, depth: usize, out: &mut BezPath) {
    if pts.len() == 2 {
        let dist = (pts[1] - pts[0]).hypot() / 3.0;
        out.curve_to(pts[0] + left * dist, pts[1] + right * dist, pts[1]);
        return;
    }

    let mut u = chord_length_parameterize(pts);
    let mut bez = generate_bezier(pts, &u, left, right);
    let (mut max_err, mut split) = max_error_of(pts, &bez, &u);

    if max_err < error {
        emit(out, &bez);
        return;
    }

    // Schneider's heuristic: if we are close, Newton-Raphson reparameterization
    // usually closes the gap without adding another segment.
    //
    // The original paper works in squared distances and writes this threshold as
    // `error * error`. We measure real distances, so that expression would be *smaller*
    // than `error` for any sub-unit tolerance and the branch would be dead code.
    // Getting this wrong is silent: the fit still converges, it just splits where it
    // should have iterated. Measured on a sampled circle, a dead band gives 18 segments
    // where an alive one gives 8.
    if max_err < error * ITERATION_BAND {
        for _ in 0..MAX_ITERATIONS {
            let u_prime = reparameterize(pts, &u, &bez);
            bez = generate_bezier(pts, &u_prime, left, right);
            let (e, s) = max_error_of(pts, &bez, &u_prime);
            u = u_prime;
            max_err = e;
            split = s;
            if max_err < error {
                emit(out, &bez);
                return;
            }
        }
    }

    if depth >= MAX_DEPTH {
        emit(out, &bez);
        return;
    }

    // Split at the worst point and fit each side, sharing a tangent so the join is smooth.
    let split = split.clamp(1, pts.len() - 2);
    let center = unit(pts[split - 1] - pts[split + 1]);
    fit_cubic(&pts[..=split], left, center, error, depth + 1, out);
    fit_cubic(&pts[split..], -center, right, error, depth + 1, out);
}

fn emit(out: &mut BezPath, bez: &[Point; 4]) {
    out.curve_to(bez[1], bez[2], bez[3]);
}

fn chord_length_parameterize(pts: &[Point]) -> Vec<f64> {
    let mut u = Vec::with_capacity(pts.len());
    u.push(0.0);
    for i in 1..pts.len() {
        let prev = u[i - 1];
        u.push(prev + (pts[i] - pts[i - 1]).hypot());
    }
    let total = *u.last().unwrap();
    if total > f64::EPSILON {
        for v in u.iter_mut() {
            *v /= total;
        }
    }
    u
}

/// Least-squares fit of one cubic to the points, with the end tangents pinned.
fn generate_bezier(pts: &[Point], u: &[f64], left: Vec2, right: Vec2) -> [Point; 4] {
    let n = pts.len();
    let first = pts[0];
    let last = pts[n - 1];

    let mut c = [[0.0f64; 2]; 2];
    let mut x = [0.0f64; 2];

    for i in 0..n {
        let t = u[i];
        let b0 = b0(t);
        let b1 = b1(t);
        let b2 = b2(t);
        let b3 = b3(t);

        let a0 = left * b1;
        let a1 = right * b2;

        c[0][0] += a0.dot(a0);
        c[0][1] += a0.dot(a1);
        c[1][0] = c[0][1];
        c[1][1] += a1.dot(a1);

        let tmp = pts[i] - (first.to_vec2() * (b0 + b1) + last.to_vec2() * (b2 + b3)).to_point();
        x[0] += a0.dot(tmp);
        x[1] += a1.dot(tmp);
    }

    let det_c = c[0][0] * c[1][1] - c[1][0] * c[0][1];
    let det_x0 = x[0] * c[1][1] - c[0][1] * x[1];
    let det_x1 = c[0][0] * x[1] - x[0] * c[1][0];

    let (mut alpha_l, mut alpha_r) = if det_c.abs() > 1e-12 {
        (det_x0 / det_c, det_x1 / det_c)
    } else {
        (0.0, 0.0)
    };

    // Degenerate or negative solutions mean the least-squares fit ran away; fall back
    // to the Wu/Barsky heuristic of one third of the chord length.
    let seg_len = (last - first).hypot();
    let epsilon = 1e-6 * seg_len;
    if alpha_l < epsilon || alpha_r < epsilon {
        alpha_l = seg_len / 3.0;
        alpha_r = seg_len / 3.0;
    }

    [first, first + left * alpha_l, last + right * alpha_r, last]
}

fn reparameterize(pts: &[Point], u: &[f64], bez: &[Point; 4]) -> Vec<f64> {
    pts.iter().zip(u.iter()).map(|(p, t)| newton_raphson(bez, *p, *t)).collect()
}

/// One Newton-Raphson step toward the parameter whose point on the curve is closest.
fn newton_raphson(bez: &[Point; 4], p: Point, t: f64) -> f64 {
    let q = eval(bez, t);

    // First and second derivative control points.
    let q1: Vec<Vec2> = (0..3).map(|i| (bez[i + 1] - bez[i]) * 3.0).collect();
    let q2: Vec<Vec2> = (0..2).map(|i| (q1[i + 1] - q1[i]) * 2.0).collect();

    let q1_t = eval_vec2(&q1, t);
    let q2_t = eval_vec1(&q2, t);

    let diff = q - p;
    let numerator = diff.dot(q1_t);
    let denominator = q1_t.dot(q1_t) + diff.dot(q2_t);

    if denominator.abs() < 1e-12 {
        t
    } else {
        (t - numerator / denominator).clamp(0.0, 1.0)
    }
}

fn eval(bez: &[Point; 4], t: f64) -> Point {
    let mt = 1.0 - t;
    let a = bez[0].to_vec2() * (mt * mt * mt);
    let b = bez[1].to_vec2() * (3.0 * mt * mt * t);
    let c = bez[2].to_vec2() * (3.0 * mt * t * t);
    let d = bez[3].to_vec2() * (t * t * t);
    (a + b + c + d).to_point()
}

fn eval_vec2(q: &[Vec2], t: f64) -> Vec2 {
    let mt = 1.0 - t;
    q[0] * (mt * mt) + q[1] * (2.0 * mt * t) + q[2] * (t * t)
}

fn eval_vec1(q: &[Vec2], t: f64) -> Vec2 {
    let mt = 1.0 - t;
    q[0] * mt + q[1] * t
}

fn max_error_of(pts: &[Point], bez: &[Point; 4], u: &[f64]) -> (f64, usize) {
    let mut worst = 0.0;
    let mut idx = pts.len() / 2;
    for i in 1..pts.len() - 1 {
        let d = (eval(bez, u[i]) - pts[i]).hypot2();
        if d > worst {
            worst = d;
            idx = i;
        }
    }
    (worst.sqrt(), idx)
}

fn b0(t: f64) -> f64 {
    let mt = 1.0 - t;
    mt * mt * mt
}
fn b1(t: f64) -> f64 {
    let mt = 1.0 - t;
    3.0 * mt * mt * t
}
fn b2(t: f64) -> f64 {
    let mt = 1.0 - t;
    3.0 * mt * t * t
}
fn b3(t: f64) -> f64 {
    t * t * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::to_svg;

    #[test]
    fn rdp_collapses_a_straight_run() {
        let pts = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [4.0, 0.0]];
        assert_eq!(simplify_rdp(&pts, 0.1), vec![[0.0, 0.0], [4.0, 0.0]]);
    }

    #[test]
    fn rdp_keeps_a_real_corner() {
        let pts = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [2.0, 2.0]];
        let out = simplify_rdp(&pts, 0.1);
        assert_eq!(out, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]]);
    }

    #[test]
    fn fitting_a_sampled_circle_stays_close_and_stays_small() {
        // Sample a circle densely, then check the fit both tracks it and compresses it.
        let samples: Vec<[f64; 2]> = (0..=180)
            .map(|i| {
                let a = (i as f64) * std::f64::consts::TAU / 180.0;
                [100.0 * a.cos(), 100.0 * a.sin()]
            })
            .collect();

        let fitted = fit_cubics(&samples, 0.05);
        let d = to_svg(&fitted);

        let segments = fitted.segments().count();
        assert!(segments <= 12, "expected a compact fit, got {segments} segments");

        // Every fitted point should still lie on the circle of radius 100.
        for ring in crate::flatten_path(&d, 0.01).unwrap() {
            for p in ring {
                let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                assert!((r - 100.0).abs() < 0.5, "point drifted off the circle: r={r}");
            }
        }
    }

    #[test]
    fn two_points_become_one_straight_segment() {
        let fitted = fit_cubics(&[[0.0, 0.0], [10.0, 0.0]], 0.1);
        assert_eq!(fitted.segments().count(), 1);
        assert_eq!(to_svg(&fitted), "M 0 0 L 10 0");
    }

    #[test]
    fn corners_are_preserved_rather_than_smoothed_across() {
        // The square's corners must survive. Fitting one cubic through all four would
        // pass through every corner and bulge far outside the shape — the bug this
        // corner-splitting exists to prevent.
        let square =
            [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0], [0.0, 0.0]];
        let fitted = fit_cubics(&square, 0.05);
        let b = crate::bounds(&to_svg(&fitted)).unwrap();
        assert!(
            (b.w - 100.0).abs() < 0.01 && (b.h - 100.0).abs() < 0.01,
            "fitting distorted a square: {b:?}"
        );
        assert!(!to_svg(&fitted).contains('C'), "straight edges should stay straight");
    }

    #[test]
    fn a_shape_with_both_corners_and_curves_keeps_each() {
        // A half-disc: a straight base and a semicircular top.
        let mut pts = vec![[-50.0, 0.0]];
        for i in 0..=90 {
            let a = std::f64::consts::PI * (1.0 - i as f64 / 90.0);
            pts.push([50.0 * a.cos(), -50.0 * a.sin()]);
        }
        let fitted = fit_cubics(&pts, 0.05);
        let d = to_svg(&fitted);
        assert!(d.contains('C'), "the arc should have been fitted to curves: {d}");
        let b = crate::bounds(&d).unwrap();
        assert!((b.w - 100.0).abs() < 0.2 && (b.h - 50.0).abs() < 0.2, "got {b:?}");
    }

    #[test]
    fn degenerate_input_does_not_panic() {
        assert_eq!(fit_cubics(&[], 0.1).elements().len(), 0);
        assert_eq!(fit_cubics(&[[1.0, 1.0]], 0.1).elements().len(), 0);
        assert_eq!(fit_cubics(&[[1.0, 1.0], [1.0, 1.0]], 0.1).elements().len(), 0);
    }
}
