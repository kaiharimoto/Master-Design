//! Affine transforms.
//!
//! Stored as the same six numbers SVG's `matrix(a b c d e f)` takes, so a node's
//! transform can go straight into a rendered attribute with no conversion, and so the
//! geometry kernel, the exporter and the editor all speak one representation.
//!
//! ```text
//! | a  c  e |   | x |
//! | b  d  f | · | y |
//! | 0  0  1 |   | 1 |
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Transform(pub [f64; 6]);

/// Rotation, scale and skew pulled back out of a matrix for the inspector.
///
/// A matrix is the right thing to *store* — it composes without loss. It is the wrong
/// thing to show a person, who wants to type "45°" into a box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decomposed {
    pub x: f64,
    pub y: f64,
    /// Radians, counter-clockwise in a y-down coordinate system.
    pub rotation: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    /// Radians of shear along x.
    pub skew_x: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn identity() -> Self {
        Transform::IDENTITY
    }

    pub fn is_identity(&self) -> bool {
        self.0
            .iter()
            .zip(Transform::IDENTITY.0.iter())
            .all(|(a, b)| (a - b).abs() < 1e-12)
    }

    pub fn translate(x: f64, y: f64) -> Self {
        Transform([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Transform([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    pub fn rotate(radians: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Transform([c, s, -s, c, 0.0, 0.0])
    }

    /// Rotate about an arbitrary point — what the editor's rotate handle needs, since
    /// people rotate around a selection's centre, not around the origin.
    pub fn rotate_about(radians: f64, cx: f64, cy: f64) -> Self {
        // Bring the pivot to the origin, turn, put it back.
        Transform::translate(-cx, -cy)
            .then(&Transform::rotate(radians))
            .then(&Transform::translate(cx, cy))
    }

    /// `self` applied first, then `other`.
    pub fn then(&self, other: &Transform) -> Transform {
        let [a1, b1, c1, d1, e1, f1] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Transform([
            a1 * a2 + b1 * c2,
            a1 * b2 + b1 * d2,
            c1 * a2 + d1 * c2,
            c1 * b2 + d1 * d2,
            e1 * a2 + f1 * c2 + e2,
            e1 * b2 + f1 * d2 + f2,
        ])
    }

    pub fn apply(&self, x: f64, y: f64) -> [f64; 2] {
        let [a, b, c, d, e, f] = self.0;
        [a * x + c * y + e, b * x + d * y + f]
    }

    pub fn determinant(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }

    /// Inverse, or `None` for a degenerate (zero-area) matrix.
    pub fn invert(&self) -> Option<Transform> {
        let det = self.determinant();
        if det.abs() < 1e-12 {
            return None;
        }
        let [a, b, c, d, e, f] = self.0;
        Some(Transform([
            d / det,
            -b / det,
            -c / det,
            a / det,
            (c * f - d * e) / det,
            (b * e - a * f) / det,
        ]))
    }

    /// Split into the translate / rotate / scale / skew a person can edit.
    pub fn decompose(&self) -> Decomposed {
        let [a, b, c, d, e, f] = self.0;

        let scale_x = (a * a + b * b).sqrt();
        let rotation = b.atan2(a);

        // Remove the rotation and x-scale, and whatever shear is left shows up as the
        // second basis vector leaning away from perpendicular.
        let shear = a * c + b * d;
        let scale_y_sq = c * c + d * d - (if scale_x > 0.0 { shear * shear / (scale_x * scale_x) } else { 0.0 });
        let scale_y = scale_y_sq.max(0.0).sqrt();
        let skew_x = if scale_x > 0.0 && scale_y > 0.0 {
            (shear / (scale_x * scale_y)).asin()
        } else {
            0.0
        };

        // A mirrored matrix has a negative determinant; report it on the y scale.
        let scale_y = if self.determinant() < 0.0 { -scale_y } else { scale_y };

        Decomposed { x: e, y: f, rotation, scale_x, scale_y, skew_x }
    }

    pub fn from_decomposed(d: &Decomposed) -> Transform {
        Transform::scale(d.scale_x, d.scale_y)
            .then(&Transform([1.0, 0.0, d.skew_x.tan(), 1.0, 0.0, 0.0]))
            .then(&Transform::rotate(d.rotation))
            .then(&Transform::translate(d.x, d.y))
    }

    /// SVG `transform` attribute value, or `None` when there is nothing to write.
    pub fn to_svg_attr(&self) -> Option<String> {
        if self.is_identity() {
            return None;
        }
        let n = |v: f64| md_geom::fmt_coord(v);
        Some(format!(
            "matrix({} {} {} {} {} {})",
            n(self.0[0]),
            n(self.0[1]),
            n(self.0[2]),
            n(self.0[3]),
            n(self.0[4]),
            n(self.0[5])
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_4;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn identity_is_skipped_in_output() {
        assert!(Transform::IDENTITY.to_svg_attr().is_none());
    }

    #[test]
    fn translate_then_scale_applies_in_order() {
        let t = Transform::translate(10.0, 0.0).then(&Transform::scale(2.0, 2.0));
        assert_eq!(t.apply(0.0, 0.0), [20.0, 0.0]);
    }

    #[test]
    fn rotate_about_a_point_leaves_that_point_alone() {
        let t = Transform::rotate_about(FRAC_PI_4, 50.0, 50.0);
        let p = t.apply(50.0, 50.0);
        assert!(close(p[0], 50.0) && close(p[1], 50.0), "got {p:?}");
    }

    #[test]
    fn invert_undoes_the_transform() {
        let t = Transform::translate(5.0, -3.0)
            .then(&Transform::rotate(0.7))
            .then(&Transform::scale(2.0, 3.0));
        let inv = t.invert().unwrap();
        let round = t.then(&inv);
        assert!(round.is_identity(), "got {round:?}");
    }

    #[test]
    fn degenerate_matrices_have_no_inverse() {
        assert!(Transform::scale(0.0, 1.0).invert().is_none());
    }

    #[test]
    fn decompose_recovers_what_was_composed() {
        let original = Decomposed {
            x: 12.0,
            y: -4.0,
            rotation: FRAC_PI_4,
            scale_x: 2.0,
            scale_y: 3.0,
            skew_x: 0.0,
        };
        let d = Transform::from_decomposed(&original).decompose();
        assert!(close(d.x, 12.0) && close(d.y, -4.0), "translation drifted: {d:?}");
        assert!(close(d.rotation, FRAC_PI_4), "rotation drifted: {d:?}");
        assert!(close(d.scale_x, 2.0) && close(d.scale_y, 3.0), "scale drifted: {d:?}");
    }

    #[test]
    fn mirroring_shows_up_as_a_negative_scale() {
        let d = Transform::scale(1.0, -1.0).decompose();
        assert!(d.scale_y < 0.0, "expected a mirrored y scale, got {d:?}");
    }
}
