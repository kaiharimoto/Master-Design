//! Tests against the animation library that actually ships.
//!
//! The manifests in `packages/md-anim-std` are data, which means nothing about them is
//! checked at compile time. This is where a typo in a shipped expression gets caught,
//! so it runs in CI on every change to that directory.

use md_anim::{apply_to_selector, bake, registry::Origin, BakeRequest, Registry};
use md_doc::node::{NodeKind, PathGeometry, RectGeometry};
use md_doc::{Document, Node, NodeId};
use serde_json::json;
use std::path::PathBuf;

fn std_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/md-anim-std/animations")
        .canonicalize()
        .expect("the standard animation library should be present in the repository")
}

fn registry() -> Registry {
    let mut reg = Registry::new();
    let loaded = reg.load_dir(&std_dir(), Origin::Standard);
    assert!(
        loaded > 0,
        "no packages loaded from {}",
        std_dir().display()
    );
    assert!(
        reg.problems.is_empty(),
        "packages failed to load: {:?}",
        reg.problems
    );
    reg
}

/// Three cards and a stroked path, which between them satisfy every package's
/// `appliesTo` constraints.
fn scene() -> (Document, Vec<NodeId>, Vec<NodeId>) {
    let mut d = Document::new("Fixture");
    d.pages[0].root.id = NodeId::from_static("nd_root");

    let mut cards = Vec::new();
    for i in 0..3 {
        let id = NodeId::parse(format!("nd_card{i}")).unwrap();
        d.pages[0].root.children.push(
            Node::new(
                id.clone(),
                NodeKind::Rect(RectGeometry {
                    width: 320.0,
                    height: 200.0,
                    corner_radius: [12.0; 4],
                }),
            )
            .with_role("card"),
        );
        cards.push(id);
    }

    let path_id = NodeId::from_static("nd_squiggle");
    d.pages[0].root.children.push(
        Node::new(
            path_id.clone(),
            NodeKind::Path(PathGeometry {
                d: "M 0 0 C 50 100 150 -100 200 0".into(),
                fill_rule: Default::default(),
            }),
        )
        .with_role("squiggle"),
    );

    (d, cards, vec![path_id])
}

#[test]
fn every_shipped_package_loads_and_validates() {
    let reg = registry();
    assert_eq!(
        reg.len(),
        6,
        "expected six standard animations, got {}",
        reg.len()
    );

    for pkg in reg.list() {
        pkg.manifest
            .validate()
            .unwrap_or_else(|e| panic!("{} is invalid: {e}", pkg.manifest.id));
        assert_eq!(pkg.manifest.namespace(), "std");
        assert!(
            !pkg.manifest.title.is_empty(),
            "{} has no title",
            pkg.manifest.id
        );
        assert!(
            !pkg.manifest.description.is_empty(),
            "{} has no description — the picker has nothing to show",
            pkg.manifest.id
        );
    }
}

#[test]
fn every_package_bakes_against_a_real_scene() {
    let reg = registry();
    let (doc, cards, paths) = scene();

    for pkg in reg.list() {
        let targets: &[NodeId] = if pkg.manifest.applies_to.accepts_kind("rect") {
            &cards
        } else {
            &paths
        };

        let timeline = bake(
            &doc,
            &BakeRequest {
                manifest: &pkg.manifest,
                targets,
                selector: "@card",
                params: Default::default(),
                trigger: None,
                timeline_id: None,
                name: None,
            },
        )
        .unwrap_or_else(|e| panic!("{} failed to bake: {e}", pkg.manifest.id));

        assert!(
            timeline.duration > 0.0,
            "{} baked a zero duration",
            pkg.manifest.id
        );
        assert!(
            !timeline.tracks.is_empty(),
            "{} baked no tracks",
            pkg.manifest.id
        );
        assert!(
            timeline.unknown_properties().is_empty(),
            "{} animates properties the exporter cannot compile: {:?}",
            pkg.manifest.id,
            timeline.unknown_properties()
        );

        for track in &timeline.tracks {
            assert!(
                track.keyframes.windows(2).all(|w| w[0].t <= w[1].t),
                "{} produced out-of-order keyframes",
                pkg.manifest.id
            );
            let first = track.keyframes.first().unwrap().t;
            let last = track.keyframes.last().unwrap().t;
            assert!(
                (0.0..=1.0).contains(&first) && (0.0..=1.0).contains(&last),
                "{} produced keyframe times outside 0..1",
                pkg.manifest.id
            );
        }
    }
}

#[test]
fn stagger_fade_up_produces_the_motion_it_describes() {
    let reg = registry();
    let (doc, _, _) = scene();

    let tl = apply_to_selector(
        &doc,
        &reg,
        "std/stagger-fade-up",
        "@card",
        json!({ "distance": 40, "duration": 0.5, "stagger": 0.1 })
            .as_object()
            .unwrap()
            .clone(),
        Some("index"),
    )
    .unwrap();

    // 0.5 + 0.1 × 2 = 0.7
    assert!((tl.duration - 0.7).abs() < 1e-9, "got {}", tl.duration);
    assert_eq!(tl.tracks.len(), 6, "three cards × opacity and translateY");

    let ty = tl
        .tracks
        .iter()
        .find(|t| t.target == NodeId::from_static("nd_card0") && t.property == "translateY")
        .unwrap();
    assert_eq!(ty.keyframes.first().unwrap().value.as_f64().unwrap(), 40.0);
    assert_eq!(ty.keyframes.last().unwrap().value.as_f64().unwrap(), 0.0);

    // The last card should still be at its start value while the first is already moving.
    let last_opacity = tl
        .tracks
        .iter()
        .find(|t| t.target == NodeId::from_static("nd_card2") && t.property == "opacity")
        .unwrap();
    assert_eq!(last_opacity.sample(0.1).unwrap().as_f64().unwrap(), 0.0);
    assert_eq!(last_opacity.sample(1.0).unwrap().as_f64().unwrap(), 1.0);
}

#[test]
fn draw_path_measures_the_actual_path_length() {
    let reg = registry();
    let (doc, _, paths) = scene();

    let tl = apply_to_selector(
        &doc,
        &reg,
        "std/draw-path",
        "@squiggle",
        Default::default(),
        Some("index"),
    )
    .unwrap();

    let track = &tl.tracks[0];
    assert_eq!(track.property, "strokeDashoffset");

    let expected =
        md_geom::path_length(&doc.node(&paths[0]).unwrap().geometry_path().unwrap()).unwrap();
    let from = track.keyframes.first().unwrap().value.as_f64().unwrap();
    assert!(
        (from - expected).abs() < 0.01,
        "dash offset should start at the path's arc length: {from} vs {expected}"
    );
    assert_eq!(track.keyframes.last().unwrap().value.as_f64().unwrap(), 0.0);
}

#[test]
fn draw_path_refuses_a_node_it_cannot_animate() {
    let reg = registry();
    let (doc, _, _) = scene();

    // Rectangles are live shapes, not paths — a dash offset on one would do nothing.
    let err = apply_to_selector(
        &doc,
        &reg,
        "std/draw-path",
        "@card",
        Default::default(),
        Some("index"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("cannot apply to a rect"), "got {err}");
}

#[test]
fn scroll_parallax_is_scroll_driven_not_time_driven() {
    let reg = registry();
    let (doc, _, _) = scene();

    let tl = apply_to_selector(
        &doc,
        &reg,
        "std/scroll-parallax",
        "@card",
        Default::default(),
        Some("index"),
    )
    .unwrap();

    assert!(
        !tl.trigger.is_time_driven(),
        "parallax must be driven by scroll position"
    );
    // Symmetric about zero, so the element sits in its designed position mid-scroll.
    let track = &tl.tracks[0];
    let start = track.keyframes.first().unwrap().value.as_f64().unwrap();
    let end = track.keyframes.last().unwrap().value.as_f64().unwrap();
    assert!(
        (start + end).abs() < 1e-9,
        "expected symmetric drift, got {start} to {end}"
    );
}

#[test]
fn a_selector_matching_nothing_is_an_error_rather_than_an_empty_timeline() {
    let reg = registry();
    let (doc, _, _) = scene();

    let err = apply_to_selector(
        &doc,
        &reg,
        "std/pop-in",
        "@nonexistent",
        Default::default(),
        Some("index"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("matched no nodes"), "got {err}");
}

#[test]
fn rebaking_picks_up_nodes_added_since_the_animation_was_applied() {
    let reg = registry();
    let (mut doc, _, _) = scene();

    let tl = apply_to_selector(
        &doc,
        &reg,
        "std/stagger-fade-up",
        "@card",
        Default::default(),
        Some("index"),
    )
    .unwrap();
    assert_eq!(tl.tracks.len(), 6);

    // A fourth card appears. Re-baking should animate it too, without anyone
    // re-selecting anything.
    doc.pages[0].root.children.push(
        Node::new(
            NodeId::from_static("nd_card3"),
            NodeKind::Rect(RectGeometry {
                width: 320.0,
                height: 200.0,
                corner_radius: [0.0; 4],
            }),
        )
        .with_role("card"),
    );

    let rebaked = md_anim::rebake(&doc, &reg, &tl, Some("index")).unwrap();
    assert_eq!(rebaked.tracks.len(), 8);
    assert_eq!(
        rebaked.id, tl.id,
        "a re-bake must replace the timeline, not add another"
    );
}

#[test]
fn parameters_survive_a_rebake() {
    let reg = registry();
    let (doc, _, _) = scene();

    let tl = apply_to_selector(
        &doc,
        &reg,
        "std/stagger-fade-up",
        "@card",
        json!({ "distance": 111 }).as_object().unwrap().clone(),
        Some("index"),
    )
    .unwrap();

    let rebaked = md_anim::rebake(&doc, &reg, &tl, Some("index")).unwrap();
    let ty = rebaked
        .tracks
        .iter()
        .find(|t| t.property == "translateY")
        .unwrap();
    assert_eq!(ty.keyframes.first().unwrap().value.as_f64().unwrap(), 111.0);
}
