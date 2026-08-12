//! Text nodes.
//!
//! Milestone 1 covers what a page actually needs: a block of text with a font, a
//! measure, alignment, and character-range overrides. The deeper typography — text on a
//! path, OpenType feature toggles, variable-font axes, optical kerning — is deliberately
//! not here yet, but the shape of [`TextSpan`] is what those will hang off, so adding
//! them later is additive rather than a rewrite.

use crate::paint::Paint;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextGeometry {
    /// The text itself. Newlines are hard breaks.
    pub content: String,

    #[serde(flatten)]
    pub font: FontSpec,

    #[serde(default, skip_serializing_if = "is_default_align")]
    pub align: TextAlign,

    /// Measure, in document units. `None` means the box hugs the text.
    ///
    /// This is the difference between Illustrator's point type and area type, and it
    /// decides whether the text wraps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,

    #[serde(default, skip_serializing_if = "is_default_vertical")]
    pub vertical_align: VerticalAlign,

    /// Character-range overrides, in code point offsets into `content`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<TextSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontSpec {
    pub font_family: String,
    pub font_size: f64,
    #[serde(default = "default_weight", skip_serializing_if = "is_default_weight")]
    pub font_weight: u16,
    #[serde(default, skip_serializing_if = "crate::paint::is_false")]
    pub italic: bool,
    /// Extra tracking, as a fraction of the font size — the unit designers think in and
    /// the one that survives a font-size change.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub letter_spacing: f64,
    /// Multiple of the font size.
    #[serde(
        default = "default_line_height",
        skip_serializing_if = "is_default_line_height"
    )]
    pub line_height: f64,
    #[serde(default, skip_serializing_if = "is_default_case")]
    pub text_case: TextCase,
    #[serde(default, skip_serializing_if = "is_default_decoration")]
    pub decoration: TextDecoration,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec {
            font_family: "Inter".to_string(),
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            letter_spacing: 0.0,
            line_height: 1.4,
            text_case: TextCase::default(),
            decoration: TextDecoration::default(),
        }
    }
}

/// An override applied to `content[start..end]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    /// Turns the range into a link in the exported page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl TextAlign {
    /// SVG's `text-anchor`, which positions relative to the anchor point rather than
    /// filling a box the way CSS `text-align` does.
    pub fn text_anchor(&self) -> &'static str {
        match self {
            TextAlign::Left | TextAlign::Justify => "start",
            TextAlign::Center => "middle",
            TextAlign::Right => "end",
        }
    }

    pub fn as_css(&self) -> &'static str {
        match self {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
            TextAlign::Justify => "justify",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VerticalAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TextCase {
    #[default]
    Original,
    Upper,
    Lower,
    Title,
}

impl TextCase {
    pub fn as_css(&self) -> Option<&'static str> {
        match self {
            TextCase::Original => None,
            TextCase::Upper => Some("uppercase"),
            TextCase::Lower => Some("lowercase"),
            TextCase::Title => Some("capitalize"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TextDecoration {
    #[default]
    None,
    Underline,
    Strikethrough,
}

fn default_weight() -> u16 {
    400
}
fn is_default_weight(v: &u16) -> bool {
    *v == 400
}
fn default_line_height() -> f64 {
    1.4
}
fn is_default_line_height(v: &f64) -> bool {
    (*v - 1.4).abs() < f64::EPSILON
}
fn is_zero(v: &f64) -> bool {
    v.abs() < f64::EPSILON
}
fn is_default_align(v: &TextAlign) -> bool {
    *v == TextAlign::default()
}
fn is_default_vertical(v: &VerticalAlign) -> bool {
    *v == VerticalAlign::default()
}
fn is_default_case(v: &TextCase) -> bool {
    *v == TextCase::default()
}
fn is_default_decoration(v: &TextDecoration) -> bool {
    *v == TextDecoration::default()
}

impl TextGeometry {
    pub fn new(content: impl Into<String>, family: impl Into<String>, size: f64) -> Self {
        TextGeometry {
            content: content.into(),
            font: FontSpec {
                font_family: family.into(),
                font_size: size,
                ..FontSpec::default()
            },
            ..TextGeometry::default()
        }
    }

    /// Lines after hard breaks, before any wrapping.
    ///
    /// Kept for callers that genuinely want the authored structure rather than the set
    /// one — a diff, say. Anything drawing or measuring should use [`Self::layout`],
    /// which also wraps.
    pub fn hard_lines(&self) -> Vec<&str> {
        self.content.split('\n').collect()
    }

    /// Applied line height in document units.
    pub fn line_height_units(&self) -> f64 {
        self.font.line_height * self.font.font_size
    }

    /// This text as `md-text` needs to see it.
    ///
    /// [`TextCase`] is applied here rather than left to CSS, because a browser's
    /// `text-transform` changes what is *drawn*, so measuring the untransformed string
    /// would size the box for text nobody sees. Uppercase is materially wider.
    pub fn run(&self) -> md_text::Run<'_> {
        md_text::Run {
            text: "",
            family: &self.font.font_family,
            size: self.font.font_size,
            weight: self.font.font_weight,
            italic: self.font.italic,
            letter_spacing: self.font.letter_spacing,
            line_height: self.font.line_height,
            max_width: self.width,
        }
    }

    /// The text as it will be drawn, with [`TextCase`] applied.
    pub fn shown_content(&self) -> std::borrow::Cow<'_, str> {
        use std::borrow::Cow;
        match self.font.text_case {
            TextCase::Original => Cow::Borrowed(&self.content),
            TextCase::Upper => Cow::Owned(self.content.to_uppercase()),
            TextCase::Lower => Cow::Owned(self.content.to_lowercase()),
            TextCase::Title => Cow::Owned(title_case(&self.content)),
        }
    }

    /// Shape, wrap and measure, using the process-wide font set.
    ///
    /// The one place text becomes geometry. Selection, auto-layout, export and snapshots
    /// all come through here, which is what stops the editor and the exported page
    /// disagreeing about where a heading ends.
    pub fn layout(&self) -> md_text::Layout {
        let shown = self.shown_content();
        md_text::shared().measure(&md_text::Run {
            text: &shown,
            ..self.run()
        })
    }
}

/// Capitalise the first letter of each word, the way CSS `text-transform: capitalize`
/// does — it touches the first letter and leaves the rest of the word alone, so `iOS`
/// stays `iOS` rather than becoming `Ios`.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at_word_start = true;
    for c in s.chars() {
        if at_word_start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            at_word_start = false;
        } else {
            out.push(c);
            if !c.is_alphanumeric() && c != '\'' && c != '’' {
                at_word_start = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_stay_out_of_the_serialized_form() {
        let t = TextGeometry::new("Hello", "Inter", 16.0);
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("fontWeight"), "got {json}");
        assert!(!json.contains("lineHeight"), "got {json}");
        assert!(json.contains("\"fontFamily\":\"Inter\""), "got {json}");
    }

    #[test]
    fn font_fields_are_flattened_not_nested() {
        let t = TextGeometry::new("Hi", "Inter", 24.0);
        let v = serde_json::to_value(&t).unwrap();
        assert!(
            v.get("fontSize").is_some(),
            "expected fontSize at the top level: {v}"
        );
        assert!(v.get("font").is_none());
    }

    #[test]
    fn hard_breaks_split_into_lines() {
        let t = TextGeometry::new("one\ntwo\nthree", "Inter", 16.0);
        assert_eq!(t.hard_lines(), vec!["one", "two", "three"]);
    }

    #[test]
    fn round_trips_through_json() {
        let mut t = TextGeometry::new("Design", "Inter", 48.0);
        t.font.font_weight = 700;
        t.align = TextAlign::Center;
        t.spans.push(TextSpan {
            start: 0,
            end: 3,
            font_weight: Some(300),
            font_family: None,
            font_size: None,
            italic: None,
            fill: None,
            href: None,
        });
        let json = serde_json::to_string(&t).unwrap();
        let back: TextGeometry = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
