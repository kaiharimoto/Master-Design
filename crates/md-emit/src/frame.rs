//! Freezing a document at a moment in its animation.
//!
//! Rendering the resting state is easy and often useless: an entrance animation's
//! resting state is "everything already arrived", so a screenshot of it says nothing
//! about the motion. To let a model *see* what it just built — mid-stagger, mid-draw —
//! the snapshot has to be taken from inside the timeline.
//!
//! Rather than teach the renderer about time, this bakes the sampled state back into a
//! copy of the document. Everything downstream stays a plain static render.

use md_doc::anim::properties;
use md_doc::{Document, NodeId, Transform};
use serde_json::Value;
use std::collections::BTreeMap;

/// A copy of `doc` with every enabled timeline on `page_key` sampled at `time` seconds
/// and applied to the nodes it animates.
///
/// Scroll-linked timelines have no clock, so `time` is read as normalized progress for
/// those — which is the only interpretation that means anything for them.
pub fn at_time(doc: &Document, page_key: &str, time: f64) -> Document {
    let mut out = doc.clone();

    let page_index = match out.page_index(page_key) {
        Some(i) => i,
        None => return out,
    };

    // Collect first, mutate second: sampling reads the page's timelines while the
    // application writes to the same page's nodes.
    let mut state: BTreeMap<NodeId, BTreeMap<String, Value>> = BTreeMap::new();
    for timeline in &out.pages[page_index].timelines {
        if !timeline.enabled {
            continue;
        }
        let sampled = if timeline.trigger.is_time_driven() {
            timeline.sample(time)
        } else {
            timeline.sample_progress(time.clamp(0.0, 1.0))
        };
        for (node, props) in sampled {
            let entry = state.entry(node).or_default();
            for (k, v) in props {
                entry.insert(k, v);
            }
        }
    }

    for (id, props) in state {
        if let Some(node) = out.pages[page_index].root.find_mut(&id) {
            apply(node, &props);
        }
    }

    out
}

fn apply(node: &mut md_doc::Node, props: &BTreeMap<String, Value>) {
    let num = |key: &str| props.get(key).and_then(|v| v.as_f64());

    if let Some(opacity) = num(properties::OPACITY) {
        // Multiplied, not replaced: an animated fade on a layer the designer already set
        // to 50% should end up at 50%, not 100%.
        node.opacity = (node.opacity * opacity).clamp(0.0, 1.0);
    }

    let tx = num(properties::TRANSLATE_X).unwrap_or(0.0);
    let ty = num(properties::TRANSLATE_Y).unwrap_or(0.0);
    let rotate = num(properties::ROTATE).unwrap_or(0.0);
    let uniform = num(properties::SCALE);
    let sx = num(properties::SCALE_X).or(uniform).unwrap_or(1.0);
    let sy = num(properties::SCALE_Y).or(uniform).unwrap_or(1.0);

    let moved = tx != 0.0 || ty != 0.0 || rotate != 0.0 || sx != 1.0 || sy != 1.0;
    if moved {
        // The export wraps animated nodes in their own element with a centred
        // transform-origin; matching that here keeps a snapshot honest about where
        // things actually end up.
        let centre = node
            .local_bounds()
            .map(|b| (b.x + b.w / 2.0, b.y + b.h / 2.0))
            .unwrap_or((0.0, 0.0));

        let local = Transform::translate(-centre.0, -centre.1)
            .then(&Transform::scale(sx, sy))
            .then(&Transform::rotate(rotate.to_radians()))
            .then(&Transform::translate(centre.0, centre.1))
            .then(&Transform::translate(tx, ty));

        node.transform = local.then(&node.transform);
    }

    if let Some(Value::String(color)) = props.get(properties::FILL) {
        if let Ok(c) = md_doc::Color::parse(color) {
            match node.fills.first_mut() {
                Some(md_doc::Paint::Solid {
                    color: existing, ..
                }) => *existing = c,
                _ => node.fills.insert(
                    0,
                    md_doc::Paint::Solid {
                        color: c,
                        opacity: 1.0,
                    },
                ),
            }
        }
    }

    if let Some(Value::String(color)) = props.get(properties::STROKE) {
        if let Ok(c) = md_doc::Color::parse(color) {
            if let Some(stroke) = node.strokes.first_mut() {
                stroke.paint = md_doc::Paint::Solid {
                    color: c,
                    opacity: 1.0,
                };
            }
        }
    }

    if let Some(width) = num(properties::STROKE_WIDTH) {
        if let Some(stroke) = node.strokes.first_mut() {
            stroke.width = width;
        }
    }

    if let Some(offset) = num(properties::STROKE_DASHOFFSET) {
        // Measured before the mutable borrow below, since it reads the same node.
        let path_length = node
            .geometry_path()
            .and_then(|d| md_geom::path_length(&d).ok());
        if let Some(stroke) = node.strokes.first_mut() {
            // A draw-on animation needs a dash pattern as long as the path to offset
            // against; without one the stroke renders solid at every time.
            if stroke.dash.is_empty() {
                if let Some(len) = path_length {
                    stroke.dash = vec![len];
                }
            }
            stroke.dash_offset = offset;
        }
    }

    if let Some(radius) = num(properties::BLUR) {
        if radius > 0.0 {
            node.effects.insert(0, md_doc::Effect::Blur { radius });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::anim::{Keyframe, Timeline, Track, Trigger};
    use md_doc::node::{NodeKind, RectGeometry};
    use md_doc::{Easing, Node, NodeId, TimelineId};
    use serde_json::json;

    fn doc() -> Document {
        let mut d = Document::new("Test");
        d.pages[0].root.id = NodeId::from_static("nd_root");
        d.pages[0].root.children.push(Node::new(
            NodeId::from_static("nd_card"),
            NodeKind::Rect(RectGeometry {
                width: 100.0,
                height: 100.0,
                corner_radius: [0.0; 4],
            }),
        ));
        d.pages[0].timelines.push(Timeline {
            id: TimelineId::from_static("tl_in"),
            name: "In".into(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: true,
            reduced_motion: Default::default(),
            source: None,
            tracks: vec![
                Track {
                    target: NodeId::from_static("nd_card"),
                    property: "opacity".into(),
                    keyframes: vec![
                        Keyframe {
                            t: 0.0,
                            value: json!(0),
                            easing: Easing::Linear,
                        },
                        Keyframe {
                            t: 1.0,
                            value: json!(1),
                            easing: Easing::Linear,
                        },
                    ],
                },
                Track {
                    target: NodeId::from_static("nd_card"),
                    property: "translateY".into(),
                    keyframes: vec![
                        Keyframe {
                            t: 0.0,
                            value: json!(100),
                            easing: Easing::Linear,
                        },
                        Keyframe {
                            t: 1.0,
                            value: json!(0),
                            easing: Easing::Linear,
                        },
                    ],
                },
            ],
        });
        d
    }

    fn card(d: &Document) -> &Node {
        d.node(&NodeId::from_static("nd_card")).unwrap()
    }

    #[test]
    fn the_start_of_an_entrance_is_invisible_and_displaced() {
        let frame = at_time(&doc(), "index", 0.0);
        let c = card(&frame);
        assert_eq!(c.opacity, 0.0);
        assert_eq!(c.transform.decompose().y, 100.0);
    }

    #[test]
    fn halfway_through_is_halfway_there() {
        let frame = at_time(&doc(), "index", 0.5);
        let c = card(&frame);
        assert!((c.opacity - 0.5).abs() < 1e-9, "got {}", c.opacity);
        assert!((c.transform.decompose().y - 50.0).abs() < 1e-9);
    }

    #[test]
    fn the_end_matches_the_designed_state() {
        let frame = at_time(&doc(), "index", 1.0);
        let c = card(&frame);
        assert_eq!(c.opacity, 1.0);
        assert!(c.transform.is_identity(), "got {:?}", c.transform);
    }

    #[test]
    fn an_animated_fade_multiplies_the_designed_opacity() {
        let mut d = doc();
        d.node_mut(&NodeId::from_static("nd_card")).unwrap().opacity = 0.5;
        let frame = at_time(&d, "index", 1.0);
        assert!(
            (card(&frame).opacity - 0.5).abs() < 1e-9,
            "an animation should not undo a design"
        );
    }

    #[test]
    fn disabled_timelines_leave_the_document_alone() {
        let mut d = doc();
        d.pages[0].timelines[0].enabled = false;
        let frame = at_time(&d, "index", 0.0);
        assert_eq!(card(&frame).opacity, 1.0);
    }

    #[test]
    fn an_unknown_page_is_returned_unchanged() {
        let d = doc();
        let frame = at_time(&d, "nope", 0.0);
        assert_eq!(frame, d);
    }
}
