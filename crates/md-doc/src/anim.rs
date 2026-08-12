//! Timelines, tracks and easing.
//!
//! ## Intent and bake
//!
//! A timeline stores two things that describe the same motion at different levels:
//!
//! * [`AnimSource`] — *what the designer asked for*: an animation package, its version,
//!   the selector it applies to, and the parameter values. This is what the inspector
//!   edits and what survives a package upgrade.
//! * [`Track`]s — *the baked result*: concrete keyframes on concrete nodes.
//!
//! Keeping both looks redundant until you notice that generators are JavaScript and the
//! exporter is Rust. Baking in the studio, where JS is available, means `md-cli export`
//! and the MCP snapshot renderer never need a JS runtime — they just read tracks. It
//! also means a document opened without its animation packages still renders correctly;
//! it just cannot re-parameterize until the package is back.
//!
//! When a parameter changes, the studio re-bakes. Nothing else regenerates tracks.

use crate::id::{NodeId, TimelineId};
use crate::paint::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub id: TimelineId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub trigger: Trigger,
    /// Seconds.
    pub duration: f64,
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_default_reduced")]
    pub reduced_motion: ReducedMotion,
    /// The editable intent this timeline was generated from, when it came from a package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AnimSource>,
    pub tracks: Vec<Track>,
}

/// What generated a timeline, and with what settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimSource {
    /// Package id, e.g. `std/stagger-fade-up`.
    pub package: String,
    /// Exact package version this was baked against, so an upgrade is a visible,
    /// deliberate act rather than a surprise the next time the app starts.
    pub version: String,
    /// Selector the package was applied to — a role, usually.
    pub target: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, Value>,
}

/// One animated property on one node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub target: NodeId,
    /// Property name — see [`properties`] for the set the exporter understands.
    pub property: String,
    pub keyframes: Vec<Keyframe>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    /// Normalized position in the timeline, 0..1.
    pub t: f64,
    pub value: Value,
    /// Easing from this keyframe to the next.
    #[serde(default, skip_serializing_if = "is_default_easing")]
    pub easing: Easing,
}

/// Property names the exporter knows how to compile.
///
/// Anything else still round-trips through the document untouched — an animation
/// package is free to invent a property — but export will report it as unhandled rather
/// than silently dropping motion.
pub mod properties {
    pub const OPACITY: &str = "opacity";
    pub const TRANSLATE_X: &str = "translateX";
    pub const TRANSLATE_Y: &str = "translateY";
    pub const SCALE: &str = "scale";
    pub const SCALE_X: &str = "scaleX";
    pub const SCALE_Y: &str = "scaleY";
    pub const ROTATE: &str = "rotate";
    pub const FILL: &str = "fill";
    pub const STROKE: &str = "stroke";
    pub const STROKE_DASHOFFSET: &str = "strokeDashoffset";
    pub const STROKE_WIDTH: &str = "strokeWidth";
    pub const BLUR: &str = "blur";

    pub const ALL: &[&str] = &[
        OPACITY,
        TRANSLATE_X,
        TRANSLATE_Y,
        SCALE,
        SCALE_X,
        SCALE_Y,
        ROTATE,
        FILL,
        STROKE,
        STROKE_DASHOFFSET,
        STROKE_WIDTH,
        BLUR,
    ];

    pub fn is_known(name: &str) -> bool {
        ALL.contains(&name)
    }

    /// Properties that compose into a single CSS `transform`, and so must be emitted
    /// together rather than as independent declarations.
    pub fn is_transform(name: &str) -> bool {
        matches!(
            name,
            TRANSLATE_X | TRANSLATE_Y | SCALE | SCALE_X | SCALE_Y | ROTATE
        )
    }
}

/// What starts a timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Trigger {
    /// Runs as soon as the page is ready.
    Load {
        #[serde(default, skip_serializing_if = "is_zero")]
        delay: f64,
    },
    /// Runs when the element scrolls into view.
    View {
        /// Fraction of the element that must be visible, 0..1.
        #[serde(
            default = "default_threshold",
            skip_serializing_if = "is_default_threshold"
        )]
        threshold: f64,
        #[serde(default = "yes", skip_serializing_if = "is_yes")]
        once: bool,
    },
    /// Progress is driven by scroll position rather than by time.
    ///
    /// `start` and `end` are viewport-relative fractions: 0 is the element's top
    /// touching the bottom of the viewport, 1 is its bottom touching the top.
    Scroll {
        #[serde(default, skip_serializing_if = "is_zero")]
        start: f64,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        end: f64,
    },
    Hover,
    Click,
    /// Loops for `iterations` passes, or forever when omitted.
    Loop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        iterations: Option<u32>,
        #[serde(default, skip_serializing_if = "crate::paint::is_false")]
        alternate: bool,
    },
}

impl Trigger {
    pub fn type_name(&self) -> &'static str {
        match self {
            Trigger::Load { .. } => "load",
            Trigger::View { .. } => "view",
            Trigger::Scroll { .. } => "scroll",
            Trigger::Hover => "hover",
            Trigger::Click => "click",
            Trigger::Loop { .. } => "loop",
        }
    }

    /// Whether progress comes from the clock or from somewhere else. Scroll-linked
    /// timelines cannot be compiled to plain CSS keyframes and need the runtime.
    pub fn is_time_driven(&self) -> bool {
        !matches!(self, Trigger::Scroll { .. })
    }
}

/// Behaviour when the visitor has asked for reduced motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReducedMotion {
    /// Jump straight to the end state. The right default: the design still arrives,
    /// it just does not travel.
    #[default]
    Skip,
    /// Play it anyway — for motion that carries meaning the page would lose.
    Play,
    /// Do not apply the timeline at all, leaving the base design.
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Easing {
    Linear,
    #[default]
    EaseInOut,
    EaseIn,
    EaseOut,
    /// Custom cubic bezier `[x1, y1, x2, y2]`, exactly as CSS spells it.
    #[serde(rename = "cubicBezier")]
    CubicBezier([f64; 4]),
    /// Hold each value until the next step.
    Steps(u32),
}

impl Easing {
    pub fn control_points(&self) -> Option<[f64; 4]> {
        match self {
            Easing::Linear => None,
            Easing::EaseIn => Some([0.42, 0.0, 1.0, 1.0]),
            Easing::EaseOut => Some([0.0, 0.0, 0.58, 1.0]),
            Easing::EaseInOut => Some([0.42, 0.0, 0.58, 1.0]),
            Easing::CubicBezier(c) => Some(*c),
            Easing::Steps(_) => None,
        }
    }

    /// CSS `animation-timing-function` value.
    pub fn as_css(&self) -> String {
        match self {
            Easing::Linear => "linear".into(),
            Easing::EaseIn => "ease-in".into(),
            Easing::EaseOut => "ease-out".into(),
            Easing::EaseInOut => "ease-in-out".into(),
            Easing::CubicBezier([a, b, c, d]) => format!(
                "cubic-bezier({}, {}, {}, {})",
                md_geom::fmt_coord(*a),
                md_geom::fmt_coord(*b),
                md_geom::fmt_coord(*c),
                md_geom::fmt_coord(*d)
            ),
            Easing::Steps(n) => format!("steps({n}, end)"),
        }
    }

    /// Map linear progress to eased progress.
    pub fn apply(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::Steps(n) => {
                let n = (*n).max(1) as f64;
                (t * n).floor() / n
            }
            _ => {
                let [x1, y1, x2, y2] = self.control_points().unwrap_or([0.0, 0.0, 1.0, 1.0]);
                let u = solve_bezier_x(t, x1, x2);
                bezier_axis(u, y1, y2)
            }
        }
    }
}

/// Value of one axis of a cubic bezier whose endpoints are pinned at 0 and 1.
fn bezier_axis(t: f64, a: f64, b: f64) -> f64 {
    let mt = 1.0 - t;
    3.0 * mt * mt * t * a + 3.0 * mt * t * t * b + t * t * t
}

/// Invert the x-axis curve to recover the bezier parameter for a given progress.
///
/// Newton-Raphson converges in a few steps for the curves CSS allows; bisection is the
/// fallback for the pathological ones where the derivative vanishes.
fn solve_bezier_x(x: f64, x1: f64, x2: f64) -> f64 {
    const EPSILON: f64 = 1e-7;

    let mut t = x;
    for _ in 0..8 {
        let err = bezier_axis(t, x1, x2) - x;
        if err.abs() < EPSILON {
            return t;
        }
        let mt = 1.0 - t;
        let d = 3.0 * mt * mt * x1 + 6.0 * mt * t * (x2 - x1) + 3.0 * t * t * (1.0 - x2);
        if d.abs() < 1e-9 {
            break;
        }
        t -= err / d;
    }

    let (mut lo, mut hi) = (0.0, 1.0);
    let mut t = x.clamp(0.0, 1.0);
    for _ in 0..24 {
        let v = bezier_axis(t, x1, x2);
        if (v - x).abs() < EPSILON {
            break;
        }
        if v < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    t
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

impl Track {
    /// Value of this track at normalized progress `p`.
    ///
    /// Returns `None` for an empty track. Progress outside 0..1 clamps to the first or
    /// last keyframe, which is what "before it started" and "after it ended" should look
    /// like for a fill-forwards animation.
    pub fn sample(&self, p: f64) -> Option<Value> {
        if self.keyframes.is_empty() {
            return None;
        }
        let first = &self.keyframes[0];
        if p <= first.t {
            return Some(first.value.clone());
        }
        let last = self.keyframes.last().unwrap();
        if p >= last.t {
            return Some(last.value.clone());
        }

        for pair in self.keyframes.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if p >= a.t && p <= b.t {
                let span = b.t - a.t;
                let local = if span.abs() < f64::EPSILON {
                    0.0
                } else {
                    (p - a.t) / span
                };
                return Some(interpolate(&a.value, &b.value, a.easing.apply(local)));
            }
        }
        Some(last.value.clone())
    }
}

impl Timeline {
    /// State of every animated property at `time` seconds.
    ///
    /// This is what lets the headless renderer produce a frame from the middle of an
    /// animation — the thing that makes `doc.snapshot` useful to a model checking its
    /// own work rather than just a picture of the resting state.
    pub fn sample(&self, time: f64) -> BTreeMap<NodeId, BTreeMap<String, Value>> {
        let p = if self.duration > 0.0 {
            (time / self.duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.sample_progress(p)
    }

    /// State at normalized progress, for scroll-linked timelines that have no clock.
    pub fn sample_progress(&self, p: f64) -> BTreeMap<NodeId, BTreeMap<String, Value>> {
        let mut out: BTreeMap<NodeId, BTreeMap<String, Value>> = BTreeMap::new();
        if !self.enabled {
            return out;
        }
        for track in &self.tracks {
            if let Some(v) = track.sample(p) {
                out.entry(track.target.clone())
                    .or_default()
                    .insert(track.property.clone(), v);
            }
        }
        out
    }

    /// Property names used here that the exporter does not know how to compile.
    pub fn unknown_properties(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .tracks
            .iter()
            .filter(|t| !properties::is_known(&t.property))
            .map(|t| t.property.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Interpolate between two keyframe values.
///
/// Numbers blend numerically, hex colours blend per channel, arrays blend element-wise,
/// and anything else snaps at the halfway point — which is the only sane behaviour for a
/// string or a boolean.
pub fn interpolate(a: &Value, b: &Value, f: f64) -> Value {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            Value::from(x + (y - x) * f)
        }
        (Value::String(x), Value::String(y)) => match (Color::parse(x), Color::parse(y)) {
            (Ok(cx), Ok(cy)) => Value::String(lerp_color(&cx, &cy, f)),
            _ => {
                if f < 0.5 {
                    a.clone()
                } else {
                    b.clone()
                }
            }
        },
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => Value::Array(
            x.iter()
                .zip(y.iter())
                .map(|(xi, yi)| interpolate(xi, yi, f))
                .collect(),
        ),
        _ => {
            if f < 0.5 {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

fn lerp_color(a: &Color, b: &Color, f: f64) -> String {
    let ca = a.rgba();
    let cb = b.rgba();
    let ch = |i: usize| {
        let v = ca[i] + (cb[i] - ca[i]) * f;
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let (r, g, bl, al) = (ch(0), ch(1), ch(2), ch(3));
    if al == 255 {
        format!("#{r:02x}{g:02x}{bl:02x}")
    } else {
        format!("#{r:02x}{g:02x}{bl:02x}{al:02x}")
    }
}

fn yes() -> bool {
    true
}
fn is_yes(v: &bool) -> bool {
    *v
}
fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    (*v - 1.0).abs() < f64::EPSILON
}
fn is_zero(v: &f64) -> bool {
    v.abs() < f64::EPSILON
}
fn default_threshold() -> f64 {
    0.2
}
fn is_default_threshold(v: &f64) -> bool {
    (*v - 0.2).abs() < f64::EPSILON
}
fn is_default_easing(v: &Easing) -> bool {
    *v == Easing::default()
}
fn is_default_reduced(v: &ReducedMotion) -> bool {
    *v == ReducedMotion::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(prop: &str, from: f64, to: f64) -> Track {
        Track {
            target: NodeId::from_static("nd_a"),
            property: prop.to_string(),
            keyframes: vec![
                Keyframe {
                    t: 0.0,
                    value: Value::from(from),
                    easing: Easing::Linear,
                },
                Keyframe {
                    t: 1.0,
                    value: Value::from(to),
                    easing: Easing::Linear,
                },
            ],
        }
    }

    #[test]
    fn linear_easing_is_the_identity() {
        for t in [0.0, 0.25, 0.5, 1.0] {
            assert!((Easing::Linear.apply(t) - t).abs() < 1e-9);
        }
    }

    #[test]
    fn easing_curves_are_pinned_at_both_ends() {
        for e in [
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::CubicBezier([0.7, 0.0, 0.3, 1.0]),
        ] {
            assert!(e.apply(0.0).abs() < 1e-6, "{e:?} did not start at 0");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-6, "{e:?} did not end at 1");
        }
    }

    #[test]
    fn ease_in_starts_slower_than_linear() {
        assert!(Easing::EaseIn.apply(0.25) < 0.25);
        assert!(Easing::EaseOut.apply(0.25) > 0.25);
    }

    #[test]
    fn steps_hold_their_value() {
        let e = Easing::Steps(4);
        assert!((e.apply(0.3) - 0.25).abs() < 1e-9);
        assert!((e.apply(0.49) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn tracks_interpolate_numbers() {
        let t = track("opacity", 0.0, 1.0);
        assert_eq!(t.sample(0.5).unwrap().as_f64().unwrap(), 0.5);
    }

    #[test]
    fn sampling_outside_the_range_clamps() {
        let t = track("opacity", 0.2, 0.8);
        assert_eq!(t.sample(-1.0).unwrap().as_f64().unwrap(), 0.2);
        assert_eq!(t.sample(5.0).unwrap().as_f64().unwrap(), 0.8);
    }

    #[test]
    fn colours_blend_per_channel() {
        let v = interpolate(&Value::from("#000000"), &Value::from("#ffffff"), 0.5);
        assert_eq!(v.as_str().unwrap(), "#808080");
    }

    #[test]
    fn non_numeric_values_snap_at_the_midpoint() {
        let a = Value::from("hello");
        let b = Value::from("world");
        assert_eq!(interpolate(&a, &b, 0.4), a);
        assert_eq!(interpolate(&a, &b, 0.6), b);
    }

    #[test]
    fn timeline_sampling_maps_time_to_progress() {
        let tl = Timeline {
            id: TimelineId::from_static("tl_a"),
            name: "fade".into(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 2.0,
            enabled: true,
            reduced_motion: ReducedMotion::Skip,
            source: None,
            tracks: vec![track("opacity", 0.0, 1.0)],
        };
        let state = tl.sample(1.0);
        let node = state.get(&NodeId::from_static("nd_a")).unwrap();
        assert_eq!(node.get("opacity").unwrap().as_f64().unwrap(), 0.5);
    }

    #[test]
    fn disabled_timelines_sample_to_nothing() {
        let tl = Timeline {
            id: TimelineId::from_static("tl_a"),
            name: String::new(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: false,
            reduced_motion: ReducedMotion::Skip,
            source: None,
            tracks: vec![track("opacity", 0.0, 1.0)],
        };
        assert!(tl.sample(0.5).is_empty());
    }

    #[test]
    fn unknown_properties_are_reported_not_swallowed() {
        let tl = Timeline {
            id: TimelineId::from_static("tl_a"),
            name: String::new(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: true,
            reduced_motion: ReducedMotion::Skip,
            source: None,
            tracks: vec![track("opacity", 0.0, 1.0), track("wobbliness", 0.0, 1.0)],
        };
        assert_eq!(tl.unknown_properties(), vec!["wobbliness".to_string()]);
    }

    #[test]
    fn triggers_round_trip() {
        for t in [
            Trigger::Load { delay: 0.5 },
            Trigger::View {
                threshold: 0.5,
                once: false,
            },
            Trigger::Scroll {
                start: 0.0,
                end: 1.0,
            },
            Trigger::Hover,
            Trigger::Loop {
                iterations: None,
                alternate: true,
            },
        ] {
            let json = serde_json::to_string(&t).unwrap();
            let back: Trigger = serde_json::from_str(&json).unwrap();
            assert_eq!(t, back, "round trip failed for {json}");
        }
    }
}
