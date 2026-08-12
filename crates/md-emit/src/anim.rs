//! Compiling timelines into what the browser can play directly.
//!
//! The document's timeline model is richer than the Web Animations API: tracks are
//! per-property, keyframe times are normalized, and several properties compose into a
//! single CSS `transform`. Resolving all of that here, in Rust, means the runtime never
//! has to — and more importantly means there is only one sampler in the system, the one
//! `md-doc` already tests.

use md_doc::anim::{properties, ReducedMotion, Timeline, Track, Trigger};
use md_doc::{Document, NodeId, Page};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// One `element.animate()` call, ready to be made.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimEntry {
    pub selector: String,
    pub keyframes: Vec<Map<String, Value>>,
    pub options: Value,
    pub trigger: Value,
    pub reduced_motion: &'static str,
}

#[derive(Debug, Default)]
pub struct CompiledAnimations {
    pub entries: Vec<AnimEntry>,
    /// Nodes any timeline touches, which need their own transform-carrying wrapper.
    pub animated: BTreeSet<String>,
    /// Nodes with a draw-on animation, whose stroke needs a dash array to offset.
    pub dashed: BTreeSet<String>,
    pub warnings: Vec<String>,
}

impl CompiledAnimations {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_payload(&self) -> String {
        serde_json::to_string(&json!({ "entries": self.entries }))
            .unwrap_or_else(|_| "{\"entries\":[]}".to_string())
    }
}

/// Compile every timeline on a page.
pub fn compile(doc: &Document, page: &Page) -> CompiledAnimations {
    let mut out = CompiledAnimations::default();

    for timeline in &page.timelines {
        if !timeline.enabled {
            continue;
        }

        let unknown = timeline.unknown_properties();
        if !unknown.is_empty() {
            out.warnings.push(format!(
                "timeline \"{}\" animates {} which the exporter cannot compile; \
                 those tracks were left out",
                timeline.name,
                unknown.join(", ")
            ));
        }

        // Tracks are per-property; the browser wants them per-element.
        let mut by_target: BTreeMap<NodeId, Vec<&Track>> = BTreeMap::new();
        for track in &timeline.tracks {
            if !properties::is_known(&track.property) || track.keyframes.is_empty() {
                continue;
            }
            if doc.node(&track.target).is_none() {
                out.warnings.push(format!(
                    "timeline \"{}\" targets {} which is not in the document",
                    timeline.name, track.target
                ));
                continue;
            }
            by_target
                .entry(track.target.clone())
                .or_default()
                .push(track);
        }

        for (target, tracks) in by_target {
            out.animated.insert(target.as_str().to_string());
            if tracks
                .iter()
                .any(|t| t.property == properties::STROKE_DASHOFFSET)
            {
                out.dashed.insert(target.as_str().to_string());
            }

            let selector = format!("[data-md-fx=\"{}\"]", target.as_str());
            let options = options_for(timeline);

            // Transform components have to travel together — `transform` is one CSS
            // property, so emitting `translateY` and `scale` as separate animations
            // would have the last one silently replace the first.
            let (transform_tracks, plain): (Vec<&Track>, Vec<&Track>) = tracks
                .into_iter()
                .partition(|t| properties::is_transform(&t.property));

            if !transform_tracks.is_empty() {
                out.entries.push(AnimEntry {
                    selector: selector.clone(),
                    keyframes: transform_keyframes(&transform_tracks),
                    options: options.clone(),
                    trigger: trigger_json(&timeline.trigger),
                    reduced_motion: reduced(timeline.reduced_motion),
                });
            }

            // Everything else is independent, so each gets its own animation and keeps
            // its own easing exactly.
            for track in plain {
                if let Some(keyframes) = plain_keyframes(track) {
                    out.entries.push(AnimEntry {
                        selector: selector.clone(),
                        keyframes,
                        options: options.clone(),
                        trigger: trigger_json(&timeline.trigger),
                        reduced_motion: reduced(timeline.reduced_motion),
                    });
                }
            }
        }
    }

    out
}

fn reduced(r: ReducedMotion) -> &'static str {
    match r {
        ReducedMotion::Skip => "skip",
        ReducedMotion::Play => "play",
        ReducedMotion::Ignore => "ignore",
    }
}

fn options_for(timeline: &Timeline) -> Value {
    let mut opts = Map::new();
    opts.insert(
        "duration".into(),
        json!((timeline.duration * 1000.0).round()),
    );
    opts.insert("fill".into(), json!("both"));

    if let Trigger::Loop {
        iterations,
        alternate,
    } = &timeline.trigger
    {
        // JSON has no Infinity; the runtime turns this sentinel back into one.
        opts.insert(
            "iterations".into(),
            match iterations {
                Some(n) => json!(n),
                None => json!("infinite"),
            },
        );
        if *alternate {
            opts.insert("direction".into(), json!("alternate"));
        }
    }

    Value::Object(opts)
}

fn trigger_json(trigger: &Trigger) -> Value {
    serde_json::to_value(trigger).unwrap_or_else(|_| json!({ "type": "load" }))
}

/// Merge transform-component tracks into one keyframe list.
///
/// Offsets are the union of every contributing track's, and each track is sampled at
/// each of them — which is exactly what `Track::sample` is for.
fn transform_keyframes(tracks: &[&Track]) -> Vec<Map<String, Value>> {
    let mut offsets: Vec<f64> = Vec::new();
    for track in tracks {
        for k in &track.keyframes {
            if !offsets.iter().any(|o| (o - k.t).abs() < 1e-9) {
                offsets.push(k.t);
            }
        }
    }
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if offsets.first().map(|f| *f > 0.0).unwrap_or(true) {
        offsets.insert(0, 0.0);
    }

    offsets
        .iter()
        .map(|offset| {
            let mut components: BTreeMap<&str, f64> = BTreeMap::new();
            for track in tracks {
                if let Some(v) = track.sample(*offset).and_then(|v| v.as_f64()) {
                    components.insert(track.property.as_str(), v);
                }
            }

            let mut frame = Map::new();
            frame.insert("offset".into(), json!(round6(*offset)));
            frame.insert("transform".into(), json!(transform_string(&components)));

            // Easing is taken from whichever contributing track defines one at this
            // offset. Components of a single transform come from one generator and share
            // a curve in practice; where they genuinely differ, split them across two
            // timelines rather than expecting one `transform` to ease two ways.
            if let Some(easing) = easing_at(tracks, *offset) {
                frame.insert("easing".into(), json!(easing));
            }
            frame
        })
        .collect()
}

fn transform_string(components: &BTreeMap<&str, f64>) -> String {
    let get = |k: &str| components.get(k).copied();
    let mut parts = Vec::new();

    let tx = get(properties::TRANSLATE_X).unwrap_or(0.0);
    let ty = get(properties::TRANSLATE_Y).unwrap_or(0.0);
    if tx != 0.0 || ty != 0.0 {
        parts.push(format!("translate({}px, {}px)", num(tx), num(ty)));
    }

    if let Some(deg) = get(properties::ROTATE) {
        if deg != 0.0 {
            parts.push(format!("rotate({}deg)", num(deg)));
        }
    }

    let uniform = get(properties::SCALE);
    let sx = get(properties::SCALE_X).or(uniform);
    let sy = get(properties::SCALE_Y).or(uniform);
    match (sx, sy) {
        (Some(x), Some(y)) if (x - y).abs() < 1e-9 => {
            if (x - 1.0).abs() > 1e-9 {
                parts.push(format!("scale({})", num(x)));
            }
        }
        (Some(x), Some(y)) => parts.push(format!("scale({}, {})", num(x), num(y))),
        (Some(x), None) => parts.push(format!("scaleX({})", num(x))),
        (None, Some(y)) => parts.push(format!("scaleY({})", num(y))),
        (None, None) => {}
    }

    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(" ")
    }
}

fn easing_at(tracks: &[&Track], offset: f64) -> Option<String> {
    for track in tracks {
        if let Some(k) = track.keyframes.iter().find(|k| (k.t - offset).abs() < 1e-9) {
            return Some(k.easing.as_css());
        }
    }
    None
}

fn plain_keyframes(track: &Track) -> Option<Vec<Map<String, Value>>> {
    let (property, unit) = match track.property.as_str() {
        properties::OPACITY => ("opacity", Unit::None),
        properties::FILL => ("fill", Unit::None),
        properties::STROKE => ("stroke", Unit::None),
        properties::STROKE_DASHOFFSET => ("strokeDashoffset", Unit::Px),
        properties::STROKE_WIDTH => ("strokeWidth", Unit::Px),
        properties::BLUR => ("filter", Unit::Blur),
        _ => return None,
    };

    let mut frames: Vec<Map<String, Value>> = Vec::with_capacity(track.keyframes.len() + 1);

    // WAAPI requires the first keyframe at offset 0. A staggered track legitimately
    // starts later, so its held opening value is repeated at zero.
    if track.keyframes.first().map(|k| k.t > 0.0).unwrap_or(false) {
        let first = &track.keyframes[0];
        let mut frame = Map::new();
        frame.insert("offset".into(), json!(0.0));
        frame.insert(property.into(), unit.apply(&first.value));
        frames.push(frame);
    }

    for k in &track.keyframes {
        let mut frame = Map::new();
        frame.insert("offset".into(), json!(round6(k.t)));
        frame.insert(property.into(), unit.apply(&k.value));
        frame.insert("easing".into(), json!(k.easing.as_css()));
        frames.push(frame);
    }

    if frames.len() < 2 {
        return None;
    }
    Some(frames)
}

enum Unit {
    None,
    Px,
    Blur,
}

impl Unit {
    fn apply(&self, v: &Value) -> Value {
        match self {
            Unit::None => v.clone(),
            Unit::Px => match v.as_f64() {
                Some(n) => json!(format!("{}px", num(n))),
                None => v.clone(),
            },
            Unit::Blur => match v.as_f64() {
                Some(n) => json!(format!("blur({}px)", num(n))),
                None => v.clone(),
            },
        }
    }
}

fn num(v: f64) -> String {
    md_geom::fmt_coord(v)
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}
