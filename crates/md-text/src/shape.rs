//! Turning characters into advances.
//!
//! The width of a string is not the sum of its characters' widths. `AV` is narrower than
//! `A` plus `V` because of a kern pair; `fi` may be one glyph; Arabic and Devanagari
//! reorder and substitute outright. Getting this wrong by a few percent is invisible
//! until a heading is centred, and then it is the only thing anyone can see.
//!
//! So shaping goes through [`rustybuzz`], a port of HarfBuzz — the same engine behind
//! the browser that will render the exported page. Measuring with anything else would
//! mean the editor and the export disagreeing by exactly the amount HarfBuzz is cleverer
//! than the alternative.

use crate::Fonts;

/// Metrics of one face, in font units scaled to a size of 1.
#[derive(Debug, Clone, Copy)]
pub struct FaceMetrics {
    /// Distance from the baseline to the top of the tallest glyph, as a multiple of the
    /// font size.
    pub ascent: f64,
    /// Below the baseline, positive downwards.
    pub descent: f64,
    /// The font's own preferred line spacing, used when a document does not state one.
    pub natural_line_height: f64,
}

impl Default for FaceMetrics {
    fn default() -> Self {
        // Roughly the proportions of a humanist sans. Only reached when no font at all
        // could be loaded, which means the numbers are already meaningless — but they
        // have to be finite, because a NaN here becomes a NaN coordinate in a path.
        FaceMetrics {
            ascent: 0.8,
            descent: 0.2,
            natural_line_height: 1.2,
        }
    }
}

/// Read a face's vertical metrics.
pub fn face_metrics(fonts: &Fonts, id: fontdb::ID) -> FaceMetrics {
    fonts
        .db()
        .with_face_data(id, |data, index| {
            let Ok(face) = ttf_parser::Face::parse(data, index) else {
                return FaceMetrics::default();
            };
            let upem = face.units_per_em() as f64;
            if upem <= 0.0 {
                return FaceMetrics::default();
            }

            let ascent = face.ascender() as f64 / upem;
            let descent = -(face.descender() as f64) / upem;
            let gap = face.line_gap() as f64 / upem;

            FaceMetrics {
                ascent,
                descent,
                natural_line_height: ascent + descent + gap,
            }
        })
        .unwrap_or_default()
}

/// Advance width of `text` at a font size of 1, before letter spacing.
///
/// Returned per-em so a caller can scale to any size without re-shaping: shaping is the
/// expensive part, and line breaking asks for widths repeatedly.
pub fn advance_per_em(fonts: &Fonts, id: fontdb::ID, text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }

    fonts
        .db()
        .with_face_data(id, |data, index| {
            let Some(face) = rustybuzz::Face::from_slice(data, index) else {
                return 0.0;
            };
            let upem = face.units_per_em() as f64;
            if upem <= 0.0 {
                return 0.0;
            }

            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(text);
            buffer.guess_segment_properties();

            let shaped = rustybuzz::shape(&face, &[], buffer);
            let total: i32 = shaped.glyph_positions().iter().map(|p| p.x_advance).sum();
            total as f64 / upem
        })
        .unwrap_or(0.0)
}

/// Width of `text` at `size`, including tracking.
///
/// Letter spacing is applied per *character* rather than per glyph, matching CSS
/// `letter-spacing`: a browser adds it after every character, including where two
/// characters became one ligature glyph.
pub fn measure_text(
    fonts: &Fonts,
    id: fontdb::ID,
    text: &str,
    size: f64,
    letter_spacing: f64,
) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let base = advance_per_em(fonts, id, text) * size;
    let tracking = letter_spacing * size * text.chars().count() as f64;
    base + tracking
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inter() -> (Fonts, fontdb::ID) {
        let fonts = Fonts::bundled();
        let id = fonts.resolve("Inter", 400, false).id.unwrap();
        (fonts, id)
    }

    #[test]
    fn a_measured_width_is_nothing_like_the_old_guess() {
        let (fonts, id) = inter();
        let text = "Design in motion";
        let measured = measure_text(&fonts, id, text, 48.0, 0.0);
        let old_guess = text.chars().count() as f64 * 48.0 * 0.55;

        assert!(measured > 0.0);
        // The point of the crate: the estimate was out by a wide margin, and always in
        // the same direction for a face like Inter.
        assert!(
            (measured - old_guess).abs() > 40.0,
            "measured {measured}, guessed {old_guess} — if these agree the shaper is not \
             actually running"
        );
    }

    #[test]
    fn width_scales_linearly_with_size() {
        let (fonts, id) = inter();
        let at_16 = measure_text(&fonts, id, "Hamburgefonstiv", 16.0, 0.0);
        let at_32 = measure_text(&fonts, id, "Hamburgefonstiv", 32.0, 0.0);
        assert!((at_32 - at_16 * 2.0).abs() < 0.01, "{at_16} vs {at_32}");
    }

    #[test]
    fn kerning_makes_a_pair_narrower_than_its_parts() {
        // The whole reason for shaping rather than summing glyph advances.
        let (fonts, id) = inter();
        let pair = advance_per_em(&fonts, id, "AV");
        let apart = advance_per_em(&fonts, id, "A") + advance_per_em(&fonts, id, "V");
        assert!(
            pair < apart,
            "no kerning was applied: AV={pair}, A+V={apart}"
        );
    }

    #[test]
    fn letter_spacing_widens_by_one_step_per_character() {
        let (fonts, id) = inter();
        let plain = measure_text(&fonts, id, "abcde", 20.0, 0.0);
        let tracked = measure_text(&fonts, id, "abcde", 20.0, 0.1);
        assert!((tracked - plain - 0.1 * 20.0 * 5.0).abs() < 0.001);
    }

    #[test]
    fn a_heavier_weight_is_wider() {
        let fonts = Fonts::bundled();
        let light = fonts.resolve("Inter", 300, false).id.unwrap();
        let heavy = fonts.resolve("Inter", 800, false).id.unwrap();
        assert!(
            measure_text(&fonts, heavy, "Weight", 32.0, 0.0)
                > measure_text(&fonts, light, "Weight", 32.0, 0.0),
            "the weight request did not reach a different face"
        );
    }

    #[test]
    fn vertical_metrics_are_plausible() {
        let (fonts, id) = inter();
        let m = face_metrics(&fonts, id);
        assert!((0.6..1.3).contains(&m.ascent), "ascent {}", m.ascent);
        assert!((0.05..0.6).contains(&m.descent), "descent {}", m.descent);
        assert!(m.natural_line_height > m.ascent);
    }

    #[test]
    fn an_empty_string_measures_zero_rather_than_erroring() {
        let (fonts, id) = inter();
        assert_eq!(measure_text(&fonts, id, "", 16.0, 0.5), 0.0);
    }
}
