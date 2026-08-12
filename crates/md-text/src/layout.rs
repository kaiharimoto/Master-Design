//! Breaking shaped text into lines.
//!
//! Greedy: fill a line until the next word will not fit, then start another. That is what
//! a browser does for `text-wrap: wrap`, and since the exported page *is* a browser
//! rendering, matching it matters more than producing prettier rag.
//!
//! Two places where the simple rule is not enough, both of which a browser also handles:
//! a single word wider than the measure has to break inside itself rather than overflow
//! forever, and a break opportunity is not only a space — a hyphen, an em dash or a slash
//! in a URL will do.

use crate::shape::{face_metrics, measure_text};
use crate::{Fonts, Run};

/// One laid-out line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    /// Advance width in document units, tracking included.
    pub width: f64,
    /// Distance from the top of the text block down to this line's baseline.
    pub baseline: f64,
}

/// The result of laying out a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub lines: Vec<Line>,
    /// Width of the widest line, or the measure if one was set.
    pub width: f64,
    /// `lines × line height` — the box the text occupies, not the ink.
    pub height: f64,
    /// Baseline offset of the first line from the top of the box.
    pub first_baseline: f64,
    pub line_height: f64,
    /// Height of the ink above a baseline, in document units.
    ///
    /// What a caller needs to place a text box against a cap height rather than a line
    /// box — aligning a heading's top edge to a rule, say — and to work out where an
    /// underline belongs.
    pub ascent: f64,
    /// Depth of the ink below a baseline, positive downwards.
    pub descent: f64,
    /// The document asked for a family this machine could not supply.
    pub substituted: bool,
}

impl Layout {
    /// An empty run still occupies one line's height — a text box with nothing typed in it
    /// has to be clickable, and a zero-height box is not.
    fn empty(
        line_height: f64,
        first_baseline: f64,
        ascent: f64,
        descent: f64,
        substituted: bool,
    ) -> Self {
        Layout {
            lines: vec![Line {
                text: String::new(),
                width: 0.0,
                baseline: first_baseline,
            }],
            width: 0.0,
            height: line_height,
            first_baseline,
            line_height,
            ascent,
            descent,
            substituted,
        }
    }
}

pub(crate) fn measure(fonts: &Fonts, run: &Run<'_>) -> Layout {
    let resolved = fonts.resolve(run.family, run.weight, run.italic);
    let line_height = (run.line_height * run.size).max(0.0);

    let Some(id) = resolved.id else {
        // No font at all. Report the geometry the document asked for rather than
        // collapsing to zero, so a box stays where the designer put it.
        let fallback = crate::shape::FaceMetrics::default();
        return Layout::empty(
            line_height,
            line_height * fallback.ascent,
            fallback.ascent * run.size,
            fallback.descent * run.size,
            true,
        );
    };

    let metrics = face_metrics(fonts, id);

    // Centre the font's own ink within the requested line height, which is what a browser
    // does with `line-height` larger than the font's natural spacing. Without this, text
    // set at 2.0 line height sits at the top of its leading rather than in the middle of
    // it, and every baseline in an export is off by the half-leading.
    let natural = metrics.natural_line_height * run.size;
    let half_leading = (line_height - natural) / 2.0;
    let first_baseline = half_leading + metrics.ascent * run.size;

    if run.text.is_empty() {
        return Layout::empty(
            line_height,
            first_baseline,
            metrics.ascent * run.size,
            metrics.descent * run.size,
            resolved.substituted,
        );
    }

    let width_of = |s: &str| measure_text(fonts, id, s, run.size, run.letter_spacing);

    let mut lines: Vec<Line> = Vec::new();
    for hard_line in run.text.split('\n') {
        match run.max_width {
            Some(measure) if measure > 0.0 => {
                for piece in wrap(hard_line, measure, &width_of) {
                    let width = width_of(&piece);
                    lines.push(Line {
                        text: piece,
                        width,
                        baseline: 0.0,
                    });
                }
            }
            _ => lines.push(Line {
                width: width_of(hard_line),
                text: hard_line.to_string(),
                baseline: 0.0,
            }),
        }
    }

    for (i, line) in lines.iter_mut().enumerate() {
        line.baseline = first_baseline + i as f64 * line_height;
    }

    let widest = lines.iter().map(|l| l.width).fold(0.0, f64::max);

    Layout {
        width: run.max_width.filter(|m| *m > 0.0).unwrap_or(widest),
        height: line_height * lines.len() as f64,
        first_baseline,
        line_height,
        ascent: metrics.ascent * run.size,
        descent: metrics.descent * run.size,
        substituted: resolved.substituted,
        lines,
    }
}

/// Greedily break one hard line to a measure.
fn wrap(text: &str, measure: f64, width_of: &dyn Fn(&str) -> f64) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();

    for chunk in break_opportunities(text) {
        // Leading whitespace on a wrapped line is dropped, as in CSS: the space that
        // caused the break is consumed by it.
        if current.is_empty() && chunk.trim_start().is_empty() {
            continue;
        }

        let candidate = format!("{current}{chunk}");
        if width_of(candidate.trim_end()) <= measure || current.is_empty() {
            current = candidate;
            continue;
        }

        out.push(current.trim_end().to_string());
        current = chunk.trim_start().to_string();
    }

    if !current.trim().is_empty() || out.is_empty() {
        out.push(current.trim_end().to_string());
    }

    // Anything still wider than the measure is one unbreakable run — a long URL, a
    // language that does not use spaces — and has to be broken inside itself or it
    // overflows the box forever.
    out.into_iter()
        .flat_map(|line| break_overlong(line, measure, width_of))
        .collect()
}

/// Split into chunks that each end at a place a line may break.
///
/// A chunk carries its trailing space, so joining chunks reproduces the original text
/// exactly and the caller can trim only where it actually breaks.
fn break_opportunities(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut in_trailing_space = false;

    for c in text.chars() {
        if c.is_whitespace() {
            current.push(c);
            in_trailing_space = true;
            continue;
        }

        if in_trailing_space {
            chunks.push(std::mem::take(&mut current));
            in_trailing_space = false;
        }

        current.push(c);

        // Break *after* these, the way a browser does: `long-hyphenated-word` may wrap at
        // a hyphen, and a URL may wrap after a slash.
        if matches!(c, '-' | '–' | '—' | '/') {
            chunks.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Break a single run that is wider than the measure, at character boundaries.
fn break_overlong(line: String, measure: f64, width_of: &dyn Fn(&str) -> f64) -> Vec<String> {
    if width_of(&line) <= measure {
        return vec![line];
    }

    let mut out = Vec::new();
    let mut current = String::new();

    for c in line.chars() {
        let candidate = format!("{current}{c}");
        if width_of(&candidate) > measure && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(c);
    }

    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<'a>(text: &'a str, size: f64, max_width: Option<f64>) -> Run<'a> {
        Run {
            text,
            size,
            max_width,
            ..Run::default()
        }
    }

    #[test]
    fn a_single_line_hugs_the_text() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("Hello", 32.0, None));
        assert_eq!(layout.lines.len(), 1);
        assert!(layout.width > 0.0);
        assert!((layout.height - 32.0 * 1.4).abs() < 0.001);
    }

    #[test]
    fn hard_breaks_are_honoured_without_a_measure() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("one\ntwo\nthree", 16.0, None));
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.lines[1].text, "two");
        // The widest line sets the width.
        assert!(layout.width >= layout.lines[2].width);
    }

    #[test]
    fn baselines_step_by_exactly_the_line_height() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("a\nb\nc", 20.0, None));
        let step = layout.lines[1].baseline - layout.lines[0].baseline;
        assert!((step - layout.line_height).abs() < 1e-9);
        assert!((layout.lines[2].baseline - layout.lines[1].baseline - step).abs() < 1e-9);
    }

    #[test]
    fn the_first_baseline_sits_inside_the_first_line() {
        // Not at the top of the box and not below it: text drawn at this baseline has to
        // land within its own line box or every export is off by the leading.
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("Hg", 40.0, None));
        assert!(layout.first_baseline > 0.0);
        assert!(layout.first_baseline < layout.line_height);
    }

    #[test]
    fn extra_leading_is_split_above_and_below_like_css() {
        let fonts = Fonts::bundled();
        let tight = fonts.measure(&Run {
            text: "Hg",
            size: 20.0,
            line_height: 1.0,
            ..Run::default()
        });
        let loose = fonts.measure(&Run {
            text: "Hg",
            size: 20.0,
            line_height: 3.0,
            ..Run::default()
        });
        let extra = loose.line_height - tight.line_height;
        assert!(
            (loose.first_baseline - tight.first_baseline - extra / 2.0).abs() < 1e-9,
            "half the extra leading should go above the first baseline"
        );
    }

    #[test]
    fn text_wraps_at_the_measure() {
        let fonts = Fonts::bundled();
        let text = "The quick brown fox jumps over the lazy dog";
        let layout = fonts.measure(&run(text, 16.0, Some(120.0)));

        assert!(layout.lines.len() > 1, "nothing wrapped");
        for line in &layout.lines {
            assert!(
                line.width <= 120.0 + 0.01,
                "{:?} is {} wide, over the 120 measure",
                line.text,
                line.width
            );
        }
        // Nothing may be lost or duplicated in the process.
        let rejoined: String = layout
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(rejoined, text);
    }

    #[test]
    fn a_measure_sets_the_width_even_when_the_text_is_shorter() {
        // Area type: the box is the measure the designer drew, not the ink inside it.
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("Hi", 16.0, Some(300.0)));
        assert_eq!(layout.width, 300.0);
    }

    #[test]
    fn a_word_wider_than_the_measure_breaks_inside_itself() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("Antidisestablishmentarianism", 32.0, Some(80.0)));
        assert!(layout.lines.len() > 1, "an overlong word never broke");
        for line in &layout.lines {
            assert!(
                line.width <= 80.0 + 0.01,
                "{:?} still overflows at {}",
                line.text,
                line.width
            );
        }
    }

    #[test]
    fn a_url_can_break_after_a_slash() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run(
            "see https://example.com/a/very/long/path for more",
            14.0,
            Some(110.0),
        ));
        assert!(layout.lines.len() > 2);
        assert!(
            layout.lines.iter().any(|l| l.text.ends_with('/')),
            "no break landed after a slash: {:?}",
            layout.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_hyphenated_word_can_break_at_its_hyphen() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("well-considered-design", 20.0, Some(120.0)));
        assert!(layout.lines.len() > 1);
        assert!(layout.lines[0].text.ends_with('-'));
    }

    #[test]
    fn empty_text_still_has_a_line_to_click_on() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("", 24.0, None));
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.width, 0.0);
        assert!(
            layout.height > 0.0,
            "an empty text box would be unclickable"
        );
    }

    #[test]
    fn a_substitution_is_reported_through_the_layout() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&Run {
            text: "Hello",
            family: "Definitely Not Installed",
            ..Run::default()
        });
        assert!(layout.substituted);
        assert!(layout.width > 0.0, "a substituted font must still measure");
    }

    #[test]
    fn wrapping_never_loses_a_trailing_word() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("alpha beta gamma delta", 16.0, Some(90.0)));
        assert!(layout.lines.last().unwrap().text.contains("delta"));
    }

    #[test]
    fn ink_metrics_come_back_in_document_units() {
        let fonts = Fonts::bundled();
        let layout = fonts.measure(&run("Hg", 100.0, None));
        // Inter's ascender is a little over the em; the descender well under it.
        assert!((60.0..130.0).contains(&layout.ascent), "{}", layout.ascent);
        assert!((5.0..60.0).contains(&layout.descent), "{}", layout.descent);
    }

    #[test]
    fn every_measurement_is_finite() {
        // A NaN width becomes a NaN coordinate, and a NaN coordinate becomes path data no
        // parser can read back.
        let fonts = Fonts::bundled();
        for size in [0.0, 0.001, 16.0, 4000.0] {
            let layout = fonts.measure(&run("Edge case", size, Some(0.0)));
            assert!(layout.width.is_finite(), "width at size {size}");
            assert!(layout.height.is_finite(), "height at size {size}");
            assert!(layout.first_baseline.is_finite(), "baseline at size {size}");
        }
    }
}
