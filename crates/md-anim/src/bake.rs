//! Turning a package plus parameters into a concrete timeline.
//!
//! Baking is where *intent* becomes *keyframes*. The resulting [`Timeline`] keeps a
//! record of what produced it in [`AnimSource`], so changing a parameter later means
//! re-baking rather than reverse-engineering. See `md_doc::anim` for why the document
//! stores both.

use crate::error::{AnimError, Result};
use crate::expr::Scope;
use crate::manifest::{AnimationManifest, Generator, ParamKind, TrackTemplate};
use md_doc::anim::{AnimSource, Easing, Keyframe, Timeline, Track, Trigger};
use md_doc::node::NodeKind;
use md_doc::{Document, NodeId, TimelineId};
use serde_json::{Map, Value};

/// Everything needed to bake one timeline.
#[derive(Debug, Clone)]
pub struct BakeRequest<'a> {
    pub manifest: &'a AnimationManifest,
    /// Nodes the animation applies to, in the order it should treat them. Stagger
    /// follows this order, so it is the caller's job to sort meaningfully — usually
    /// document order, which is what the selector returns.
    pub targets: &'a [NodeId],
    /// The selector that produced `targets`, recorded so the animation can be re-run
    /// against a changed document.
    pub selector: &'a str,
    pub params: Map<String, Value>,
    pub trigger: Option<Trigger>,
    pub timeline_id: Option<TimelineId>,
    pub name: Option<String>,
}

/// Bake a package into a timeline against a document.
pub fn bake(doc: &Document, req: &BakeRequest) -> Result<Timeline> {
    let manifest = req.manifest;
    manifest.validate()?;

    let generator = match &manifest.generator {
        Generator::Declarative(g) => g,
        Generator::Script { entry } => {
            return Err(AnimError::ScriptGenerator {
                id: manifest.id.clone(),
                entry: entry.clone(),
            })
        }
    };

    validate_targets(doc, manifest, req.targets)?;
    let params = merge_params(manifest, &req.params)?;

    let mut scope = numeric_scope(manifest, &params);
    scope.insert("count".into(), req.targets.len() as f64);

    let duration = generator.duration.eval(&scope)?;
    if duration <= 0.0 {
        return Err(AnimError::Bake {
            id: manifest.id.clone(),
            reason: format!("duration evaluated to {duration}, which must be positive"),
        });
    }

    let default_easing = default_easing(manifest, &params);
    let mut tracks = Vec::new();

    for (index, target) in req.targets.iter().enumerate() {
        let mut local = scope.clone();
        local.insert("index".into(), index as f64);
        add_node_variables(doc, target, &mut local);

        let start = generator.per_target.start_at.eval(&local)?;
        let end = generator.per_target.end_at.eval(&local)?;
        if end <= start {
            return Err(AnimError::Bake {
                id: manifest.id.clone(),
                reason: format!("target {index} ends at {end}s but starts at {start}s"),
            });
        }

        let t0 = (start / duration).clamp(0.0, 1.0);
        let t1 = (end / duration).clamp(0.0, 1.0);

        for template in &generator.per_target.tracks {
            tracks.push(build_track(
                manifest,
                template,
                target,
                &params,
                &local,
                t0,
                t1,
                default_easing,
            )?);
        }
    }

    Ok(Timeline {
        id: req.timeline_id.clone().unwrap_or_else(TimelineId::new),
        name: req.name.clone().unwrap_or_else(|| manifest.title.clone()),
        trigger: req.trigger.clone().unwrap_or_else(|| manifest.default_trigger.clone()),
        duration,
        enabled: true,
        reduced_motion: Default::default(),
        source: Some(AnimSource {
            package: manifest.id.clone(),
            version: manifest.version.clone(),
            target: req.selector.to_string(),
            params,
        }),
        tracks,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_track(
    manifest: &AnimationManifest,
    template: &TrackTemplate,
    target: &NodeId,
    params: &Map<String, Value>,
    scope: &Scope,
    t0: f64,
    t1: f64,
    default_easing: Easing,
) -> Result<Track> {
    let from = resolve_value(&template.from, params, scope)?;
    let to = resolve_value(&template.to, params, scope)?;

    let easing = match &template.easing {
        Some(v) => {
            let resolved = resolve_value(v, params, scope)?;
            serde_json::from_value::<Easing>(resolved.clone()).map_err(|_| AnimError::Bake {
                id: manifest.id.clone(),
                reason: format!("'{resolved}' is not a valid easing"),
            })?
        }
        None => default_easing,
    };

    let mut keyframes = Vec::with_capacity(3);
    // Hold the start value until this target's turn comes around. Without this, a
    // staggered element would already be interpolating before its delay elapsed.
    if t0 > 0.0 {
        keyframes.push(Keyframe { t: 0.0, value: from.clone(), easing: Easing::Linear });
    }
    keyframes.push(Keyframe { t: t0, value: from, easing });
    keyframes.push(Keyframe { t: t1, value: to, easing: Easing::Linear });

    Ok(Track { target: target.clone(), property: template.property.clone(), keyframes })
}

/// Resolve a manifest value.
///
/// * numbers pass through
/// * `"$key"` substitutes a parameter, whatever its type — this is how colours and
///   strings reach a track
/// * `"=expr"` evaluates arithmetic
/// * anything else is a literal
fn resolve_value(v: &Value, params: &Map<String, Value>, scope: &Scope) -> Result<Value> {
    match v {
        Value::String(s) => {
            if let Some(key) = s.strip_prefix('$') {
                params
                    .get(key)
                    .cloned()
                    .ok_or_else(|| AnimError::UnknownParamReference { key: key.to_string() })
            } else if let Some(formula) = s.strip_prefix('=') {
                Ok(Value::from(crate::expr::eval(formula, scope)?))
            } else {
                Ok(v.clone())
            }
        }
        _ => Ok(v.clone()),
    }
}

fn validate_targets(
    doc: &Document,
    manifest: &AnimationManifest,
    targets: &[NodeId],
) -> Result<()> {
    let rules = &manifest.applies_to;

    if targets.len() < rules.min_targets {
        return Err(AnimError::TargetMismatch {
            id: manifest.id.clone(),
            reason: format!(
                "needs at least {} target(s), got {}",
                rules.min_targets,
                targets.len()
            ),
        });
    }
    if let Some(max) = rules.max_targets {
        if targets.len() > max {
            return Err(AnimError::TargetMismatch {
                id: manifest.id.clone(),
                reason: format!("accepts at most {max} target(s), got {}", targets.len()),
            });
        }
    }

    for id in targets {
        let node = doc.node(id).ok_or_else(|| AnimError::TargetMismatch {
            id: manifest.id.clone(),
            reason: format!("no node {id} in this document"),
        })?;
        let kind = node.kind.type_name();
        if !rules.accepts_kind(kind) {
            return Err(AnimError::TargetMismatch {
                id: manifest.id.clone(),
                reason: format!(
                    "cannot apply to a {kind} ({id}); it accepts {}",
                    rules.kinds.join(", ")
                ),
            });
        }
    }

    Ok(())
}

/// Fill in defaults for anything the caller left out, and reject anything unknown or
/// out of range.
fn merge_params(
    manifest: &AnimationManifest,
    supplied: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    for key in supplied.keys() {
        if manifest.param(key).is_none() {
            let known: Vec<&str> = manifest.params.iter().map(|p| p.key.as_str()).collect();
            return Err(AnimError::BadParam {
                key: key.clone(),
                reason: format!("not a parameter of {} (has: {})", manifest.id, known.join(", ")),
            });
        }
    }

    let mut out = Map::new();
    for spec in &manifest.params {
        let value = supplied.get(&spec.key).cloned().unwrap_or_else(|| spec.default.clone());
        spec.validate(&value)?;
        out.insert(spec.key.clone(), value);
    }
    Ok(out)
}

/// Parameters an expression can reference.
///
/// Booleans come through as 1 and 0 so a manifest can write `distance * enabled`
/// without needing conditionals in the expression language.
fn numeric_scope(manifest: &AnimationManifest, params: &Map<String, Value>) -> Scope {
    let mut scope = Scope::new();
    for spec in &manifest.params {
        if let Some(v) = params.get(&spec.key) {
            match v {
                Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        scope.insert(spec.key.clone(), f);
                    }
                }
                Value::Bool(b) => {
                    scope.insert(spec.key.clone(), if *b { 1.0 } else { 0.0 });
                }
                _ => {}
            }
        }
    }
    scope
}

/// Per-node values an expression can reference.
///
/// `pathLength` is the reason this exists: a draw-on animation has to know how long the
/// stroke is to set a dash offset, and that is geometry, not a parameter.
fn add_node_variables(doc: &Document, id: &NodeId, scope: &mut Scope) {
    let node = match doc.node(id) {
        Some(n) => n,
        None => return,
    };

    if let Some(b) = node.local_bounds() {
        scope.insert("width".into(), b.w);
        scope.insert("height".into(), b.h);
    }

    let d = node.transform.decompose();
    scope.insert("x".into(), d.x);
    scope.insert("y".into(), d.y);

    if let Some(path) = node.geometry_path() {
        if let Ok(len) = md_geom::path_length(&path) {
            scope.insert("pathLength".into(), len);
        }
    }

    // Index within the parent, so a manifest can stagger by position on the page rather
    // than by selection order.
    if let Some(parent) = doc.parent_of(id) {
        if let Some(i) = parent.children.iter().position(|c| &c.id == id) {
            scope.insert("siblingIndex".into(), i as f64);
        }
    }

    scope.entry("pathLength".into()).or_insert(0.0);
    scope.entry("width".into()).or_insert(0.0);
    scope.entry("height".into()).or_insert(0.0);
    scope.entry("siblingIndex".into()).or_insert(0.0);
}

/// The first easing-typed parameter becomes the timeline's default easing.
fn default_easing(manifest: &AnimationManifest, params: &Map<String, Value>) -> Easing {
    manifest
        .params
        .iter()
        .find(|p| matches!(p.kind, ParamKind::Easing))
        .and_then(|p| params.get(&p.key))
        .and_then(|v| serde_json::from_value::<Easing>(v.clone()).ok())
        .unwrap_or(Easing::EaseInOut)
}

/// Which node kinds a package can animate, as a human-readable phrase for error
/// messages and the picker UI.
pub fn kind_summary(manifest: &AnimationManifest) -> String {
    if manifest.applies_to.kinds.iter().any(|k| k == "*") {
        "any layer".to_string()
    } else {
        manifest.applies_to.kinds.join(", ")
    }
}

/// Convenience: does this node kind accept this package?
pub fn accepts(manifest: &AnimationManifest, kind: &NodeKind) -> bool {
    manifest.applies_to.accepts_kind(kind.type_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{AppliesTo, Category, DeclarativeGenerator, Expr, ParamSpec, PerTarget};
    use md_doc::node::{NodeKind, RectGeometry};
    use md_doc::{Node, NodeId};
    use serde_json::json;

    fn manifest() -> AnimationManifest {
        AnimationManifest {
            id: "std/fade-up".into(),
            version: "1.0.0".into(),
            title: "Fade Up".into(),
            description: String::new(),
            category: Category::Entrance,
            tags: vec![],
            applies_to: AppliesTo::default(),
            default_trigger: Trigger::View { threshold: 0.2, once: true },
            params: vec![
                ParamSpec {
                    key: "distance".into(),
                    label: "Distance".into(),
                    kind: ParamKind::Number {
                        min: Some(0.0),
                        max: Some(400.0),
                        step: Some(1.0),
                        unit: "px".into(),
                    },
                    default: json!(32),
                    description: String::new(),
                },
                ParamSpec {
                    key: "duration".into(),
                    label: "Duration".into(),
                    kind: ParamKind::Number {
                        min: Some(0.05),
                        max: Some(5.0),
                        step: Some(0.05),
                        unit: "s".into(),
                    },
                    default: json!(0.6),
                    description: String::new(),
                },
                ParamSpec {
                    key: "stagger".into(),
                    label: "Stagger".into(),
                    kind: ParamKind::Number {
                        min: Some(0.0),
                        max: Some(2.0),
                        step: Some(0.01),
                        unit: "s".into(),
                    },
                    default: json!(0.08),
                    description: String::new(),
                },
                ParamSpec {
                    key: "easing".into(),
                    label: "Easing".into(),
                    kind: ParamKind::Easing,
                    default: json!("easeOut"),
                    description: String::new(),
                },
            ],
            generator: Generator::Declarative(DeclarativeGenerator {
                duration: Expr::Formula("duration + stagger * (count - 1)".into()),
                per_target: PerTarget {
                    start_at: Expr::Formula("stagger * index".into()),
                    end_at: Expr::Formula("stagger * index + duration".into()),
                    tracks: vec![
                        TrackTemplate {
                            property: "opacity".into(),
                            from: json!(0),
                            to: json!(1),
                            easing: None,
                        },
                        TrackTemplate {
                            property: "translateY".into(),
                            from: json!("=distance"),
                            to: json!(0),
                            easing: None,
                        },
                    ],
                },
            }),
        }
    }

    fn doc_with(n: usize) -> (Document, Vec<NodeId>) {
        let mut d = Document::new("Test");
        d.pages[0].root.id = NodeId::from_static("nd_root");
        let ids: Vec<NodeId> =
            (0..n).map(|i| NodeId::parse(format!("nd_{i}")).unwrap()).collect();
        for id in &ids {
            d.pages[0].root.children.push(Node::new(
                id.clone(),
                NodeKind::Rect(RectGeometry {
                    width: 100.0,
                    height: 50.0,
                    corner_radius: [0.0; 4],
                }),
            ));
        }
        (d, ids)
    }

    fn request<'a>(
        m: &'a AnimationManifest,
        targets: &'a [NodeId],
        params: Value,
    ) -> BakeRequest<'a> {
        BakeRequest {
            manifest: m,
            targets,
            selector: "@card",
            params: params.as_object().cloned().unwrap_or_default(),
            trigger: None,
            timeline_id: Some(TimelineId::from_static("tl_test")),
            name: None,
        }
    }

    #[test]
    fn baking_produces_one_track_per_property_per_target() {
        let m = manifest();
        let (doc, ids) = doc_with(3);
        let tl = bake(&doc, &request(&m, &ids, json!({}))).unwrap();
        assert_eq!(tl.tracks.len(), 6, "3 targets × 2 properties");
    }

    #[test]
    fn stagger_offsets_each_target_in_turn() {
        let m = manifest();
        let (doc, ids) = doc_with(3);
        let tl = bake(&doc, &request(&m, &ids, json!({}))).unwrap();

        // duration = 0.6 + 0.08 * 2 = 0.76
        assert!((tl.duration - 0.76).abs() < 1e-9, "got {}", tl.duration);

        // The first target starts immediately; the third starts at 0.16s = t 0.2105.
        let first = tl.tracks.iter().find(|t| t.target == ids[0]).unwrap();
        assert_eq!(first.keyframes[0].t, 0.0);

        let third = tl.tracks.iter().find(|t| t.target == ids[2]).unwrap();
        let start = third.keyframes.iter().find(|k| k.t > 0.0).unwrap();
        assert!((start.t - 0.16 / 0.76).abs() < 1e-6, "got {}", start.t);
    }

    #[test]
    fn a_staggered_target_holds_its_start_value_until_its_turn() {
        let m = manifest();
        let (doc, ids) = doc_with(3);
        let tl = bake(&doc, &request(&m, &ids, json!({}))).unwrap();

        let third_opacity = tl
            .tracks
            .iter()
            .find(|t| t.target == ids[2] && t.property == "opacity")
            .unwrap();

        // Without the hold keyframe this would already be fading at t=0.05.
        assert_eq!(third_opacity.sample(0.05).unwrap().as_f64().unwrap(), 0.0);
    }

    #[test]
    fn expressions_read_parameters() {
        let m = manifest();
        let (doc, ids) = doc_with(1);
        let tl = bake(&doc, &request(&m, &ids, json!({ "distance": 120 }))).unwrap();

        let ty = tl.tracks.iter().find(|t| t.property == "translateY").unwrap();
        assert_eq!(ty.keyframes[0].value.as_f64().unwrap(), 120.0);
        assert_eq!(ty.keyframes.last().unwrap().value.as_f64().unwrap(), 0.0);
    }

    #[test]
    fn the_easing_parameter_becomes_the_track_easing() {
        let m = manifest();
        let (doc, ids) = doc_with(1);
        let tl = bake(&doc, &request(&m, &ids, json!({ "easing": "linear" }))).unwrap();
        assert_eq!(tl.tracks[0].keyframes[0].easing, Easing::Linear);
    }

    #[test]
    fn the_source_records_what_to_re_bake_from() {
        let m = manifest();
        let (doc, ids) = doc_with(2);
        let tl = bake(&doc, &request(&m, &ids, json!({ "distance": 64 }))).unwrap();

        let src = tl.source.unwrap();
        assert_eq!(src.package, "std/fade-up");
        assert_eq!(src.version, "1.0.0");
        assert_eq!(src.target, "@card");
        assert_eq!(src.params.get("distance").unwrap().as_f64().unwrap(), 64.0);
        // Defaults are recorded too, so a re-bake cannot drift if a default changes.
        assert!(src.params.contains_key("duration"));
    }

    #[test]
    fn unknown_parameters_are_rejected_with_the_valid_list() {
        let m = manifest();
        let (doc, ids) = doc_with(1);
        let err = bake(&doc, &request(&m, &ids, json!({ "wobble": 3 }))).unwrap_err().to_string();
        assert!(err.contains("wobble"), "got {err}");
        assert!(err.contains("distance"), "should list valid params: {err}");
    }

    #[test]
    fn out_of_range_parameters_are_rejected() {
        let m = manifest();
        let (doc, ids) = doc_with(1);
        assert!(bake(&doc, &request(&m, &ids, json!({ "distance": 9999 }))).is_err());
    }

    #[test]
    fn kind_restrictions_are_enforced() {
        let mut m = manifest();
        m.applies_to.kinds = vec!["path".into()];
        let (doc, ids) = doc_with(1);
        let err = bake(&doc, &request(&m, &ids, json!({}))).unwrap_err().to_string();
        assert!(err.contains("cannot apply to a rect"), "got {err}");
    }

    #[test]
    fn missing_targets_are_reported() {
        let m = manifest();
        let (doc, _) = doc_with(1);
        let ghost = vec![NodeId::from_static("nd_ghost")];
        assert!(bake(&doc, &request(&m, &ghost, json!({}))).is_err());
    }

    #[test]
    fn node_geometry_is_available_to_expressions() {
        let mut m = manifest();
        if let Generator::Declarative(g) = &mut m.generator {
            g.per_target.tracks = vec![TrackTemplate {
                property: "translateX".into(),
                from: json!("=width"),
                to: json!(0),
                easing: None,
            }];
        }
        let (doc, ids) = doc_with(1);
        let tl = bake(&doc, &request(&m, &ids, json!({}))).unwrap();
        assert_eq!(tl.tracks[0].keyframes[0].value.as_f64().unwrap(), 100.0);
    }

    #[test]
    fn script_generators_are_refused_here_with_a_clear_reason() {
        let mut m = manifest();
        m.generator = Generator::Script { entry: "index.js".into() };
        let (doc, ids) = doc_with(1);
        let err = bake(&doc, &request(&m, &ids, json!({}))).unwrap_err().to_string();
        assert!(err.contains("script"), "got {err}");
    }

    #[test]
    fn baked_timelines_only_use_properties_the_exporter_knows() {
        let m = manifest();
        let (doc, ids) = doc_with(2);
        let tl = bake(&doc, &request(&m, &ids, json!({}))).unwrap();
        assert!(tl.unknown_properties().is_empty(), "got {:?}", tl.unknown_properties());
    }
}
