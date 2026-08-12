//! Parsing, normalization and measurement of SVG path data.

use crate::{fmt_coord, GeomError, DEFAULT_TOLERANCE};
use kurbo::{
    Affine, BezPath, CubicBez, ParamCurve, ParamCurveArclen, PathEl, Point, QuadBez, Shape,
};
use serde::{Deserialize, Serialize};

/// Axis-aligned bounding box, in document units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    pub const ZERO: Bounds = Bounds {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    pub fn from_kurbo(r: kurbo::Rect) -> Self {
        Bounds {
            x: r.x0,
            y: r.y0,
            w: r.width(),
            h: r.height(),
        }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    /// Grow the box on every side. Used to give stroke width and touch slop some room.
    pub fn inflate(&self, by: f64) -> Bounds {
        Bounds {
            x: self.x - by,
            y: self.y - by,
            w: self.w + by * 2.0,
            h: self.h + by * 2.0,
        }
    }

    pub fn union(&self, other: &Bounds) -> Bounds {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        Bounds {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// Parse SVG path data into a bezier path.
pub fn parse(d: &str) -> Result<BezPath, GeomError> {
    BezPath::from_svg(d).map_err(|e| GeomError::ParsePath(e.to_string()))
}

/// Serialize a bezier path back to SVG path data using our canonical number format.
///
/// We write our own serializer rather than using `BezPath::to_svg` because the number
/// formatting has to match `md-doc`'s canonical form exactly — otherwise every trip
/// through the geometry kernel would dirty the file.
pub fn to_svg(path: &BezPath) -> String {
    let mut out = String::new();
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => {
                push_cmd(&mut out, 'M');
                push_pt(&mut out, *p);
            }
            PathEl::LineTo(p) => {
                push_cmd(&mut out, 'L');
                push_pt(&mut out, *p);
            }
            PathEl::QuadTo(c, p) => {
                push_cmd(&mut out, 'Q');
                push_pt(&mut out, *c);
                out.push(' ');
                push_pt(&mut out, *p);
            }
            PathEl::CurveTo(c1, c2, p) => {
                push_cmd(&mut out, 'C');
                push_pt(&mut out, *c1);
                out.push(' ');
                push_pt(&mut out, *c2);
                out.push(' ');
                push_pt(&mut out, *p);
            }
            PathEl::ClosePath => push_cmd(&mut out, 'Z'),
        }
    }
    out
}

fn push_cmd(out: &mut String, c: char) {
    if !out.is_empty() {
        out.push(' ');
    }
    out.push(c);
    if c != 'Z' {
        out.push(' ');
    }
}

fn push_pt(out: &mut String, p: Point) {
    out.push_str(&fmt_coord(p.x));
    out.push(' ');
    out.push_str(&fmt_coord(p.y));
}

/// Rewrite path data into the normal form the document format stores: absolute
/// coordinates, cubic segments only, `M`/`C`/`Z`.
///
/// Normalizing on write means two paths that describe the same shape are the same
/// string, so the AI and the GUI cannot produce competing spellings of one geometry.
pub fn normalize(d: &str) -> Result<String, GeomError> {
    let path = parse(d)?;
    Ok(to_svg(&to_cubics(&path)))
}

/// Convert every segment to a cubic, leaving `MoveTo`/`ClosePath` alone.
pub fn to_cubics(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut cur = Point::ZERO;
    let mut start = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.move_to(p);
                cur = p;
                start = p;
            }
            PathEl::LineTo(p) => {
                let c1 = cur.lerp(p, 1.0 / 3.0);
                let c2 = cur.lerp(p, 2.0 / 3.0);
                out.curve_to(c1, c2, p);
                cur = p;
            }
            PathEl::QuadTo(c, p) => {
                let cubic: CubicBez = QuadBez::new(cur, c, p).raise();
                out.curve_to(cubic.p1, cubic.p2, cubic.p3);
                cur = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                out.curve_to(c1, c2, p);
                cur = p;
            }
            PathEl::ClosePath => {
                out.close_path();
                cur = start;
            }
        }
    }
    out
}

/// Tight-ish bounding box of the path's geometry, ignoring stroke.
pub fn bounds(d: &str) -> Result<Bounds, GeomError> {
    let path = parse(d)?;
    if path.elements().is_empty() {
        return Err(GeomError::EmptyPath);
    }
    Ok(Bounds::from_kurbo(path.bounding_box()))
}

/// Apply an affine matrix `[a, b, c, d, e, f]` — the same six numbers SVG's
/// `matrix()` takes, and the same six the document stores on every node.
pub fn transform_path(d: &str, m: [f64; 6]) -> Result<String, GeomError> {
    let mut path = parse(d)?;
    path.apply_affine(Affine::new(m));
    Ok(to_svg(&path))
}

/// Flatten to polylines, one per subpath.
///
/// Closed subpaths come back with their first point repeated at the end, which is what
/// the polygon code downstream expects.
pub fn flatten_path(d: &str, tolerance: f64) -> Result<Vec<Vec<[f64; 2]>>, GeomError> {
    let path = parse(d)?;
    Ok(flatten_bez(&path, tolerance))
}

pub(crate) fn flatten_bez(path: &BezPath, tolerance: f64) -> Vec<Vec<[f64; 2]>> {
    let tolerance = if tolerance > 0.0 {
        tolerance
    } else {
        DEFAULT_TOLERANCE
    };
    let mut subpaths: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    let mut start: Option<[f64; 2]> = None;

    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            let pt = [p.x, p.y];
            start = Some(pt);
            current.push(pt);
        }
        PathEl::LineTo(p) => current.push([p.x, p.y]),
        PathEl::ClosePath => {
            if let Some(s) = start {
                if current.last() != Some(&s) {
                    current.push(s);
                }
            }
            if current.len() > 1 {
                subpaths.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        // `kurbo::flatten` only ever emits the three cases above.
        _ => {}
    });

    if current.len() > 1 {
        subpaths.push(current);
    }
    subpaths
}

/// Total arc length of every segment in the path.
///
/// The animation library leans on this: `draw-path` needs a stroke dash length, and
/// motion-along-path needs to map progress to distance.
pub fn path_length(d: &str) -> Result<f64, GeomError> {
    let path = parse(d)?;
    Ok(path
        .segments()
        .map(|seg| seg.arclen(DEFAULT_TOLERANCE))
        .sum())
}

/// Point and unit tangent at a given distance along the path.
pub fn point_at_length(d: &str, distance: f64) -> Result<([f64; 2], [f64; 2]), GeomError> {
    let path = parse(d)?;
    let segs: Vec<_> = path.segments().collect();
    if segs.is_empty() {
        return Err(GeomError::EmptyPath);
    }

    let total: f64 = segs.iter().map(|s| s.arclen(DEFAULT_TOLERANCE)).sum();
    let target = distance.clamp(0.0, total);

    let mut walked = 0.0;
    for seg in &segs {
        let len = seg.arclen(DEFAULT_TOLERANCE);
        if walked + len >= target || (walked + len - target).abs() < f64::EPSILON {
            let local = (target - walked).clamp(0.0, len);
            let t = seg.inv_arclen(local, DEFAULT_TOLERANCE);
            let p = seg.eval(t);
            // Sample slightly ahead for the tangent; cheaper and steadier than
            // differentiating each segment kind separately.
            let ahead = seg.eval((t + 1e-4).min(1.0));
            let behind = seg.eval((t - 1e-4).max(0.0));
            let dx = ahead.x - behind.x;
            let dy = ahead.y - behind.y;
            let mag = (dx * dx + dy * dy).sqrt();
            let tangent = if mag > f64::EPSILON {
                [dx / mag, dy / mag]
            } else {
                [1.0, 0.0]
            };
            return Ok(([p.x, p.y], tangent));
        }
        walked += len;
    }

    let seg = segs.last().unwrap();
    let p = seg.eval(1.0);
    Ok(([p.x, p.y], [1.0, 0.0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_turns_lines_into_cubics() {
        let out = normalize("M 0 0 L 10 0 Z").unwrap();
        assert!(out.starts_with("M 0 0 C"), "got {out}");
        assert!(out.ends_with('Z'), "got {out}");
    }

    #[test]
    fn normalize_is_idempotent() {
        let once = normalize("M0,0 Q 5 10 10 0 Z").unwrap();
        let twice = normalize(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn bounds_of_a_unit_square() {
        let b = bounds("M 0 0 L 10 0 L 10 10 L 0 10 Z").unwrap();
        assert_eq!(
            b,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0
            }
        );
    }

    #[test]
    fn transform_translates() {
        let out = transform_path("M 0 0 L 10 0", [1.0, 0.0, 0.0, 1.0, 5.0, 7.0]).unwrap();
        assert_eq!(out, "M 5 7 L 15 7");
    }

    #[test]
    fn closed_subpaths_flatten_to_rings() {
        let rings = flatten_path("M 0 0 L 10 0 L 10 10 Z", 0.01).unwrap();
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].first(), rings[0].last());
    }

    #[test]
    fn length_of_a_straight_line() {
        let len = path_length("M 0 0 L 10 0").unwrap();
        assert!((len - 10.0).abs() < 1e-6, "got {len}");
    }

    #[test]
    fn point_at_length_walks_the_line() {
        let (p, t) = point_at_length("M 0 0 L 10 0", 2.5).unwrap();
        assert!((p[0] - 2.5).abs() < 1e-4, "got {p:?}");
        assert!((t[0] - 1.0).abs() < 1e-4, "got {t:?}");
    }
}
