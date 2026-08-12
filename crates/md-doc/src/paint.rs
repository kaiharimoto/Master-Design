//! Colour, fills, strokes and effects.
//!
//! Colours are stored as hex strings rather than numeric triples. That is a deliberate
//! trade: a struct would be marginally faster to work with, but `"#ff0055"` is what a
//! designer reads, what CSS accepts verbatim, and what an AI model writes correctly
//! without being taught anything. Validation on parse keeps it honest.

use crate::error::{DocError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// An sRGB colour with optional alpha, normalized to lowercase `#rrggbb` or
/// `#rrggbbaa`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color(String);

impl Color {
    pub const BLACK: &'static str = "#000000";

    pub fn parse(s: &str) -> Result<Self> {
        let t = s.trim().to_ascii_lowercase();
        let hex = t.strip_prefix('#').ok_or_else(|| DocError::InvalidColor(s.to_string()))?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(DocError::InvalidColor(s.to_string()));
        }

        let expanded = match hex.len() {
            // #rgb and #rgba double each digit, the same way CSS does.
            3 | 4 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 | 8 => hex.to_string(),
            _ => return Err(DocError::InvalidColor(s.to_string())),
        };

        // Fully opaque is the common case; drop a redundant `ff` so two spellings of
        // the same colour cannot produce two different files.
        let expanded = if expanded.len() == 8 && expanded.ends_with("ff") {
            expanded[..6].to_string()
        } else {
            expanded
        };

        Ok(Color(format!("#{expanded}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Channels in 0..1, alpha included.
    pub fn rgba(&self) -> [f64; 4] {
        let h = &self.0[1..];
        let byte = |i: usize| {
            u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0) as f64 / 255.0
        };
        [byte(0), byte(1), byte(2), if h.len() == 8 { byte(3) } else { 1.0 }]
    }

    /// The opaque `#rrggbb` part, for SVG attributes that take colour and opacity
    /// separately.
    pub fn hex_rgb(&self) -> String {
        self.0[..7].to_string()
    }

    /// Alpha in 0..1.
    pub fn alpha(&self) -> f64 {
        self.rgba()[3]
    }
}

impl Default for Color {
    fn default() -> Self {
        Color(Self::BLACK.to_string())
    }
}

impl TryFrom<String> for Color {
    type Error = DocError;
    fn try_from(s: String) -> Result<Self> {
        Color::parse(&s)
    }
}

impl From<Color> for String {
    fn from(c: Color) -> String {
        c.0
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One stop in a gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientStop {
    /// Position along the gradient, 0..1.
    pub offset: f64,
    pub color: Color,
}

fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    (*v - 1.0).abs() < f64::EPSILON
}
pub(crate) fn is_false(v: &bool) -> bool {
    !*v
}

/// How a region is painted.
///
/// Gradient coordinates are in *object space*: `[0, 0]` is the top-left of the shape's
/// bounding box and `[1, 1]` the bottom-right. That is what makes a gradient survive
/// being resized, and it is what SVG's `objectBoundingBox` units give us for free.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Paint {
    Solid {
        color: Color,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f64,
    },
    LinearGradient {
        from: [f64; 2],
        to: [f64; 2],
        stops: Vec<GradientStop>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f64,
    },
    RadialGradient {
        center: [f64; 2],
        radius: f64,
        stops: Vec<GradientStop>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f64,
    },
    /// Reference to an entry in the project's `assets/` directory.
    Image {
        asset: String,
        #[serde(default)]
        fit: ImageFit,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        opacity: f64,
    },
}

impl Paint {
    pub fn solid(hex: &str) -> Result<Paint> {
        Ok(Paint::Solid { color: Color::parse(hex)?, opacity: 1.0 })
    }

    pub fn opacity(&self) -> f64 {
        match self {
            Paint::Solid { opacity, .. }
            | Paint::LinearGradient { opacity, .. }
            | Paint::RadialGradient { opacity, .. }
            | Paint::Image { opacity, .. } => *opacity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ImageFit {
    #[default]
    Cover,
    Contain,
    Fill,
    /// Repeat at natural size.
    Tile,
}

/// A stroke, reusing the geometry kernel's cap/join vocabulary so the two never drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    pub paint: Paint,
    pub width: f64,
    #[serde(default, skip_serializing_if = "is_default_cap")]
    pub cap: md_geom::LineCap,
    #[serde(default, skip_serializing_if = "is_default_join")]
    pub join: md_geom::LineJoin,
    #[serde(default = "default_miter", skip_serializing_if = "is_default_miter")]
    pub miter_limit: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dash: Vec<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dash_offset: f64,
    #[serde(default, skip_serializing_if = "is_default_align")]
    pub align: StrokeAlign,
}

/// Where the stroke sits relative to the path.
///
/// SVG only knows `Center`. `Inside` and `Outside` are resolved at export time by
/// outlining the stroke and clipping, which is why the geometry kernel exposes
/// `outline_stroke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum StrokeAlign {
    #[default]
    Center,
    Inside,
    Outside,
}

fn default_miter() -> f64 {
    4.0
}
fn is_default_miter(v: &f64) -> bool {
    (*v - 4.0).abs() < f64::EPSILON
}
fn is_zero(v: &f64) -> bool {
    v.abs() < f64::EPSILON
}
fn is_default_cap(v: &md_geom::LineCap) -> bool {
    *v == md_geom::LineCap::default()
}
fn is_default_join(v: &md_geom::LineJoin) -> bool {
    *v == md_geom::LineJoin::default()
}
fn is_default_align(v: &StrokeAlign) -> bool {
    *v == StrokeAlign::default()
}

impl Stroke {
    pub fn new(paint: Paint, width: f64) -> Self {
        Stroke {
            paint,
            width,
            cap: Default::default(),
            join: Default::default(),
            miter_limit: 4.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            align: Default::default(),
        }
    }

    pub fn style(&self) -> md_geom::StrokeStyle {
        md_geom::StrokeStyle {
            width: self.width,
            cap: self.cap,
            join: self.join,
            miter_limit: self.miter_limit,
            dash: self.dash.clone(),
            dash_offset: self.dash_offset,
        }
    }
}

/// Post-processing applied to a node's rendered result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Effect {
    Blur {
        radius: f64,
    },
    DropShadow {
        dx: f64,
        dy: f64,
        blur: f64,
        color: Color,
    },
    InnerShadow {
        dx: f64,
        dy: f64,
        blur: f64,
        color: Color,
    },
}

/// Compositing mode, spelled the way CSS `mix-blend-mode` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    pub fn as_css(&self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::ColorDodge => "color-dodge",
            BlendMode::ColorBurn => "color-burn",
            BlendMode::HardLight => "hard-light",
            BlendMode::SoftLight => "soft-light",
            BlendMode::Difference => "difference",
            BlendMode::Exclusion => "exclusion",
            BlendMode::Hue => "hue",
            BlendMode::Saturation => "saturation",
            BlendMode::Color => "color",
            BlendMode::Luminosity => "luminosity",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_hex_expands() {
        assert_eq!(Color::parse("#f05").unwrap().as_str(), "#ff0055");
    }

    #[test]
    fn opaque_alpha_is_dropped_so_one_colour_has_one_spelling() {
        assert_eq!(Color::parse("#ff0055ff").unwrap(), Color::parse("#ff0055").unwrap());
    }

    #[test]
    fn real_alpha_is_kept() {
        let c = Color::parse("#FF005580").unwrap();
        assert_eq!(c.as_str(), "#ff005580");
        assert!((c.alpha() - 0.502).abs() < 0.01);
    }

    #[test]
    fn case_is_normalized() {
        assert_eq!(Color::parse("#AABBCC").unwrap().as_str(), "#aabbcc");
    }

    #[test]
    fn junk_is_rejected() {
        // Note `#ff00` is absent: four digits is valid `#rgba` shorthand, not junk.
        for bad in ["ff0055", "#gg0055", "#12345", "#", "rebeccapurple", "#1234567"] {
            assert!(Color::parse(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn channels_come_back_as_unit_floats() {
        let c = Color::parse("#ff8000").unwrap();
        let [r, g, b, a] = c.rgba();
        assert!((r - 1.0).abs() < 1e-9);
        assert!((g - 0.502).abs() < 0.01);
        assert!(b.abs() < 1e-9);
        assert!((a - 1.0).abs() < 1e-9);
    }

    #[test]
    fn default_paint_opacity_stays_out_of_the_file() {
        let p = Paint::solid("#123456").unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("opacity"), "got {json}");
    }
}
