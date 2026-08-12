//! Converting a stroked path into a filled outline.
//!
//! Needed in three places: the "outline stroke" command in the editor, boolean
//! operations against stroked artwork, and accurate hit-testing of dashed strokes.

use crate::{parse, path::to_svg, GeomError, DEFAULT_TOLERANCE};
use serde::{Deserialize, Serialize};

/// Stroke appearance, mirroring the fields the document format stores on a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_offset: f64,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// Expand a stroked path into the filled region it covers.
pub fn outline_stroke(d: &str, style: &StrokeStyle) -> Result<String, GeomError> {
    if style.width <= 0.0 {
        return Err(GeomError::InvalidParam(
            "stroke width must be positive".into(),
        ));
    }

    let path = parse(d)?;

    let mut k = kurbo::Stroke::new(style.width)
        .with_caps(match style.cap {
            LineCap::Butt => kurbo::Cap::Butt,
            LineCap::Round => kurbo::Cap::Round,
            LineCap::Square => kurbo::Cap::Square,
        })
        .with_join(match style.join {
            LineJoin::Miter => kurbo::Join::Miter,
            LineJoin::Round => kurbo::Join::Round,
            LineJoin::Bevel => kurbo::Join::Bevel,
        })
        .with_miter_limit(style.miter_limit);

    // A dash array of all zeros would make the dasher spin, so require a positive entry.
    if !style.dash.is_empty() && style.dash.iter().any(|v| *v > 0.0) {
        k = k.with_dashes(style.dash_offset, style.dash.iter().copied());
    }

    let outlined = kurbo::stroke(path, &k, &kurbo::StrokeOpts::default(), DEFAULT_TOLERANCE);
    Ok(to_svg(&outlined))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds;

    #[test]
    fn outlining_a_line_produces_a_band_of_the_stroke_width() {
        let style = StrokeStyle {
            width: 4.0,
            ..Default::default()
        };
        let out = outline_stroke("M 0 0 L 10 0", &style).unwrap();
        let b = bounds(&out).unwrap();
        assert!(
            (b.h - 4.0).abs() < 0.1,
            "expected a 4-unit tall band, got {b:?}"
        );
        assert!(
            (b.w - 10.0).abs() < 0.1,
            "expected a 10-unit wide band, got {b:?}"
        );
    }

    #[test]
    fn square_caps_extend_past_the_ends() {
        let style = StrokeStyle {
            width: 4.0,
            cap: LineCap::Square,
            ..Default::default()
        };
        let out = outline_stroke("M 0 0 L 10 0", &style).unwrap();
        let b = bounds(&out).unwrap();
        assert!(
            (b.w - 14.0).abs() < 0.1,
            "caps should add half a width per end, got {b:?}"
        );
    }

    #[test]
    fn zero_width_is_rejected() {
        let style = StrokeStyle {
            width: 0.0,
            ..Default::default()
        };
        assert!(outline_stroke("M 0 0 L 10 0", &style).is_err());
    }
}
