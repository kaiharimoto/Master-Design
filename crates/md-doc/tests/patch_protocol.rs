//! Integration tests for the patch protocol.
//!
//! These exercise the contract that both editors depend on: an op either applies
//! completely and returns an inverse that exactly undoes it, or it fails and changes
//! nothing. Everything else in the system is built on that holding.

use md_doc::document::Page;
use md_doc::history::Session;
use md_doc::node::{EllipseGeometry, FrameGeometry, NodeKind, RectGeometry};
use md_doc::patch::apply_ops;
use md_doc::text::TextGeometry;
use md_doc::{
    to_canonical_string, Document, Node, NodeId, Op, PageId, Paint, Transform,
};
use md_geom::BoolOp;
use serde_json::json;

fn rect(id: &'static str, w: f64, h: f64) -> Node {
    Node::new(
        NodeId::from_static(id),
        NodeKind::Rect(RectGeometry { width: w, height: h, corner_radius: [0.0; 4] }),
    )
}

fn ellipse(id: &'static str, w: f64, h: f64) -> Node {
    Node::new(
        NodeId::from_static(id),
        NodeKind::Ellipse(EllipseGeometry { width: w, height: h }),
    )
}

fn frame(id: &'static str) -> Node {
    Node::new(
        NodeId::from_static(id),
        NodeKind::Frame(FrameGeometry {
            width: 100.0,
            height: 100.0,
            clip: false,
            corner_radius: [0.0; 4],
            layout: None,
        }),
    )
}

/// A document with a root frame containing a group of two rects.
fn doc() -> Document {
    let mut d = Document::new("Test");
    d.pages[0].root.id = NodeId::from_static("nd_root");
    let group = Node::new(NodeId::from_static("nd_group"), NodeKind::Group)
        .with_children(vec![rect("nd_a", 10.0, 10.0), rect("nd_b", 20.0, 20.0)]);
    d.pages[0].root.children.push(group);
    d
}

/// Apply ops, then apply the returned inverse, and assert we are exactly where we began.
///
/// This is the single most important property in the crate, so it gets a helper rather
/// than being spelled out in every test.
fn assert_reversible(mut d: Document, ops: Vec<Op>) -> Document {
    let before = to_canonical_string(&d).unwrap();

    let (inverse, report) = apply_ops(&mut d, &ops).expect("patch should apply");
    assert_eq!(report.applied, ops.len());
    let after = to_canonical_string(&d).unwrap();
    assert_ne!(before, after, "the patch claimed to apply but changed nothing");

    let mut undone = d.clone();
    apply_ops(&mut undone, &inverse).expect("inverse should apply");
    assert_eq!(
        to_canonical_string(&undone).unwrap(),
        before,
        "inverse ops did not restore the original document"
    );

    d
}

// ---------------------------------------------------------------------------
// node.insert / node.delete
// ---------------------------------------------------------------------------

#[test]
fn insert_appends_by_default_and_is_reversible() {
    let d = assert_reversible(
        doc(),
        vec![Op::NodeInsert {
            parent: NodeId::from_static("nd_group"),
            index: None,
            node: rect("nd_new", 5.0, 5.0),
        }],
    );
    let group = d.node(&NodeId::from_static("nd_group")).unwrap();
    assert_eq!(group.children.len(), 3);
    assert_eq!(group.children[2].id.as_str(), "nd_new");
}

#[test]
fn insert_honours_an_explicit_index() {
    let mut d = doc();
    apply_ops(
        &mut d,
        &[Op::NodeInsert {
            parent: NodeId::from_static("nd_group"),
            index: Some(0),
            node: rect("nd_new", 5.0, 5.0),
        }],
    )
    .unwrap();
    let group = d.node(&NodeId::from_static("nd_group")).unwrap();
    assert_eq!(group.children[0].id.as_str(), "nd_new");
}

#[test]
fn insert_rejects_a_duplicate_id_anywhere_in_the_subtree() {
    let mut d = doc();
    let colliding = Node::new(NodeId::from_static("nd_wrapper"), NodeKind::Group)
        .with_children(vec![rect("nd_a", 1.0, 1.0)]);

    let err = apply_ops(
        &mut d,
        &[Op::NodeInsert {
            parent: NodeId::from_static("nd_group"),
            index: None,
            node: colliding,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("nd_a"), "got {err}");
}

#[test]
fn insert_into_a_leaf_is_refused() {
    let mut d = doc();
    let err = apply_ops(
        &mut d,
        &[Op::NodeInsert {
            parent: NodeId::from_static("nd_a"),
            index: None,
            node: rect("nd_new", 1.0, 1.0),
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("cannot contain children"), "got {err}");
}

#[test]
fn insert_past_the_end_is_refused_rather_than_clamped() {
    let mut d = doc();
    assert!(apply_ops(
        &mut d,
        &[Op::NodeInsert {
            parent: NodeId::from_static("nd_group"),
            index: Some(99),
            node: rect("nd_new", 1.0, 1.0),
        }],
    )
    .is_err());
}

#[test]
fn delete_takes_the_whole_subtree_and_restores_it_in_place() {
    let d = assert_reversible(doc(), vec![Op::NodeDelete { id: NodeId::from_static("nd_group") }]);
    assert!(d.node(&NodeId::from_static("nd_a")).is_none());
    assert!(d.node(&NodeId::from_static("nd_group")).is_none());
}

#[test]
fn deleting_a_middle_child_restores_to_the_same_index() {
    let d = doc();
    let restored = {
        let mut working = d.clone();
        let (inverse, _) =
            apply_ops(&mut working, &[Op::NodeDelete { id: NodeId::from_static("nd_a") }]).unwrap();
        apply_ops(&mut working, &inverse).unwrap();
        working
    };
    let group = restored.node(&NodeId::from_static("nd_group")).unwrap();
    assert_eq!(group.children[0].id.as_str(), "nd_a", "node came back in the wrong position");
}

#[test]
fn the_page_root_cannot_be_deleted() {
    let mut d = doc();
    assert!(apply_ops(&mut d, &[Op::NodeDelete { id: NodeId::from_static("nd_root") }]).is_err());
}

// ---------------------------------------------------------------------------
// node.update
// ---------------------------------------------------------------------------

#[test]
fn update_sets_a_scalar_and_reverses() {
    let d = assert_reversible(
        doc(),
        vec![Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "width".into(),
            value: json!(250.0),
        }],
    );
    match &d.node(&NodeId::from_static("nd_a")).unwrap().kind {
        NodeKind::Rect(r) => assert_eq!(r.width, 250.0),
        other => panic!("expected a rect, got {other:?}"),
    }
}

#[test]
fn update_reaches_into_arrays() {
    let mut d = doc();
    d.node_mut(&NodeId::from_static("nd_a")).unwrap().fills.push(Paint::solid("#000000").unwrap());

    let d = assert_reversible(
        d,
        vec![Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "fills.0.color".into(),
            value: json!("#ff0055"),
        }],
    );

    let fills = &d.node(&NodeId::from_static("nd_a")).unwrap().fills;
    match &fills[0] {
        Paint::Solid { color, .. } => assert_eq!(color.as_str(), "#ff0055"),
        other => panic!("expected a solid fill, got {other:?}"),
    }
}

#[test]
fn update_validates_against_the_typed_model() {
    let mut d = doc();
    d.node_mut(&NodeId::from_static("nd_a")).unwrap().fills.push(Paint::solid("#000000").unwrap());

    // "chartreuse" is not a hex colour; the round-trip through Color must reject it.
    let err = apply_ops(
        &mut d,
        &[Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "fills.0.color".into(),
            value: json!("chartreuse"),
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("fills.0.color"), "got {err}");
}

#[test]
fn update_refuses_to_change_an_id() {
    let mut d = doc();
    let err = apply_ops(
        &mut d,
        &[Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "id".into(),
            value: json!("nd_somethingelse"),
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("cannot be changed"), "got {err}");
}

#[test]
fn a_misspelled_property_is_an_error_not_a_silent_no_op() {
    let mut d = doc();
    // The failure mode this guards against: a stray `fil1s` key that looks applied,
    // reports success, and does nothing.
    let err = apply_ops(
        &mut d,
        &[Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "fil1s.0.color".into(),
            value: json!("#ff0055"),
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("fil1s"), "got {err}");
}

#[test]
fn clearing_an_optional_property_with_null_reverses_correctly() {
    let mut d = doc();
    d.node_mut(&NodeId::from_static("nd_a")).unwrap().a11y =
        Some(md_doc::A11y { label: Some("Card".into()), ..Default::default() });

    let d = assert_reversible(
        d,
        vec![Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "a11y".into(),
            value: json!(null),
        }],
    );
    assert!(d.node(&NodeId::from_static("nd_a")).unwrap().a11y.is_none());
}

#[test]
fn setting_a_property_that_was_absent_reverses_by_clearing_it() {
    // `roles` is skipped when empty, so it is genuinely absent from the serialized node.
    let d = assert_reversible(
        doc(),
        vec![Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "roles".into(),
            value: json!(["card"]),
        }],
    );
    assert_eq!(d.node(&NodeId::from_static("nd_a")).unwrap().roles, vec!["card".to_string()]);
}

#[test]
fn transform_components_are_addressable_individually() {
    let d = assert_reversible(
        doc(),
        vec![Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "transform".into(),
            value: json!([1.0, 0.0, 0.0, 1.0, 40.0, 12.0]),
        }],
    );
    let t = d.node(&NodeId::from_static("nd_a")).unwrap().transform;
    assert_eq!(t.apply(0.0, 0.0), [40.0, 12.0]);
}

// ---------------------------------------------------------------------------
// node.move
// ---------------------------------------------------------------------------

#[test]
fn move_reparents_and_reverses_to_the_original_slot() {
    let mut d = doc();
    d.pages[0].root.children.push(frame("nd_target"));

    let d = assert_reversible(
        d,
        vec![Op::NodeMove {
            id: NodeId::from_static("nd_a"),
            parent: NodeId::from_static("nd_target"),
            index: None,
        }],
    );

    assert_eq!(d.parent_of(&NodeId::from_static("nd_a")).unwrap().id.as_str(), "nd_target");
    assert_eq!(d.node(&NodeId::from_static("nd_group")).unwrap().children.len(), 1);
}

#[test]
fn reordering_within_a_parent_is_just_a_move() {
    let mut d = doc();
    apply_ops(
        &mut d,
        &[Op::NodeMove {
            id: NodeId::from_static("nd_a"),
            parent: NodeId::from_static("nd_group"),
            index: Some(1),
        }],
    )
    .unwrap();

    let kids: Vec<&str> = d
        .node(&NodeId::from_static("nd_group"))
        .unwrap()
        .children
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(kids, vec!["nd_b", "nd_a"], "bring-to-front did not reorder");
}

#[test]
fn moving_a_node_into_its_own_descendant_is_refused() {
    let mut d = doc();
    // nd_group contains nd_a; moving nd_group into nd_a would detach the whole subtree.
    let mut inner = doc();
    let _ = &mut inner;

    let err = apply_ops(
        &mut d,
        &[Op::NodeMove {
            id: NodeId::from_static("nd_group"),
            parent: NodeId::from_static("nd_a"),
            index: None,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("descendant"), "got {err}");
}

#[test]
fn moving_into_a_leaf_is_refused() {
    let mut d = doc();
    assert!(apply_ops(
        &mut d,
        &[Op::NodeMove {
            id: NodeId::from_static("nd_b"),
            parent: NodeId::from_static("nd_a"),
            index: None,
        }],
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// path.boolean
// ---------------------------------------------------------------------------

#[test]
fn boolean_union_replaces_operands_with_one_path_and_reverses() {
    let mut d = doc();
    {
        let group = d.node_mut(&NodeId::from_static("nd_group")).unwrap();
        group.children.clear();
        let mut a = ellipse("nd_a", 100.0, 100.0);
        a.fills.push(Paint::solid("#ff0055").unwrap());
        a.name = "Blob".into();
        let mut b = ellipse("nd_b", 100.0, 100.0);
        b.transform = Transform::translate(60.0, 0.0);
        group.children.push(a);
        group.children.push(b);
    }

    let d = assert_reversible(
        d,
        vec![Op::PathBoolean {
            ids: vec![NodeId::from_static("nd_a"), NodeId::from_static("nd_b")],
            mode: BoolOp::Union,
            result_id: Some(NodeId::from_static("nd_merged")),
        }],
    );

    assert!(d.node(&NodeId::from_static("nd_a")).is_none());
    assert!(d.node(&NodeId::from_static("nd_b")).is_none());

    let merged = d.node(&NodeId::from_static("nd_merged")).unwrap();
    assert!(matches!(merged.kind, NodeKind::Path(_)));
    // Appearance and name carry over from the first operand.
    assert_eq!(merged.name, "Blob");
    assert_eq!(merged.fills.len(), 1);

    // The union of two 100-wide circles offset by 60 spans 160.
    let bounds = merged.local_bounds().unwrap();
    assert!((bounds.w - 160.0).abs() < 1.0, "got {bounds:?}");
}

#[test]
fn boolean_accounts_for_operand_transforms() {
    let mut d = doc();
    {
        let group = d.node_mut(&NodeId::from_static("nd_group")).unwrap();
        group.children.clear();
        group.children.push(rect("nd_a", 100.0, 100.0));
        let mut b = rect("nd_b", 100.0, 100.0);
        b.transform = Transform::translate(500.0, 0.0);
        group.children.push(b);
    }

    let mut working = d.clone();
    apply_ops(
        &mut working,
        &[Op::PathBoolean {
            ids: vec![NodeId::from_static("nd_a"), NodeId::from_static("nd_b")],
            mode: BoolOp::Union,
            result_id: Some(NodeId::from_static("nd_merged")),
        }],
    )
    .unwrap();

    // If the transform were ignored, both squares would sit at the origin and the union
    // would be 100 wide instead of 600.
    let bounds = working.node(&NodeId::from_static("nd_merged")).unwrap().local_bounds().unwrap();
    assert!((bounds.w - 600.0).abs() < 1.0, "operand transform was dropped: {bounds:?}");
}

#[test]
fn boolean_needs_at_least_two_nodes() {
    let mut d = doc();
    assert!(apply_ops(
        &mut d,
        &[Op::PathBoolean {
            ids: vec![NodeId::from_static("nd_a")],
            mode: BoolOp::Union,
            result_id: None,
        }],
    )
    .is_err());
}

#[test]
fn boolean_requires_a_shared_parent() {
    let mut d = doc();
    d.pages[0].root.children.push(rect("nd_outside", 10.0, 10.0));

    let err = apply_ops(
        &mut d,
        &[Op::PathBoolean {
            ids: vec![NodeId::from_static("nd_a"), NodeId::from_static("nd_outside")],
            mode: BoolOp::Union,
            result_id: None,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("share a parent"), "got {err}");
}

#[test]
fn boolean_refuses_nodes_with_no_outline() {
    let mut d = doc();
    {
        let group = d.node_mut(&NodeId::from_static("nd_group")).unwrap();
        group.children.push(Node::new(
            NodeId::from_static("nd_text"),
            NodeKind::Text(TextGeometry::new("hi", "Inter", 16.0)),
        ));
    }
    let err = apply_ops(
        &mut d,
        &[Op::PathBoolean {
            ids: vec![NodeId::from_static("nd_a"), NodeId::from_static("nd_text")],
            mode: BoolOp::Union,
            result_id: None,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("no outline"), "got {err}");
}

// ---------------------------------------------------------------------------
// Pages, timelines and tokens
// ---------------------------------------------------------------------------

#[test]
fn adding_a_page_is_reversible() {
    let page = Page::new(PageId::from_static("pg_about"), "About", "about", 1440.0, 900.0);
    let d = assert_reversible(doc(), vec![Op::PageInsert { page: Box::new(page), index: None }]);
    assert_eq!(d.pages.len(), 2);
}

#[test]
fn a_duplicate_slug_is_refused() {
    let mut d = doc();
    let page = Page::new(PageId::from_static("pg_dupe"), "Home again", "index", 100.0, 100.0);
    assert!(apply_ops(&mut d, &[Op::PageInsert { page: Box::new(page), index: None }]).is_err());
}

#[test]
fn the_last_page_cannot_be_deleted() {
    let mut d = doc();
    assert!(apply_ops(&mut d, &[Op::PageDelete { page: "index".into() }]).is_err());
}

#[test]
fn page_properties_update_and_reverse() {
    let d = assert_reversible(
        doc(),
        vec![Op::PageUpdate {
            page: "index".into(),
            path: "name".into(),
            value: json!("Landing"),
        }],
    );
    assert_eq!(d.page("index").unwrap().name, "Landing");
}

#[test]
fn page_update_will_not_be_used_to_smuggle_scene_graph_edits() {
    let mut d = doc();
    let err = apply_ops(
        &mut d,
        &[Op::PageUpdate {
            page: "index".into(),
            path: "root.name".into(),
            value: json!("hacked"),
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("node.*"), "got {err}");
}

#[test]
fn tokens_can_be_set_before_their_group_exists() {
    let d = assert_reversible(
        doc(),
        vec![Op::TokensSet { path: "colors.accent".into(), value: json!("#ff0055") }],
    );
    assert_eq!(d.tokens.colors.get("accent").unwrap().as_str(), "#ff0055");
}

#[test]
fn an_invalid_token_value_is_refused() {
    let mut d = doc();
    assert!(apply_ops(
        &mut d,
        &[Op::TokensSet { path: "colors.accent".into(), value: json!("not-a-colour") }],
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// Atomicity and the wire format
// ---------------------------------------------------------------------------

#[test]
fn a_multi_op_patch_that_fails_late_applies_nothing() {
    let mut d = doc();
    let before = to_canonical_string(&d).unwrap();

    let result = apply_ops(
        &mut d,
        &[
            Op::NodeInsert {
                parent: NodeId::from_static("nd_group"),
                index: None,
                node: rect("nd_new", 1.0, 1.0),
            },
            Op::NodeUpdate {
                id: NodeId::from_static("nd_a"),
                path: "width".into(),
                value: json!(99.0),
            },
            Op::NodeDelete { id: NodeId::from_static("nd_does_not_exist") },
        ],
    );

    assert!(result.is_err());
    assert_eq!(to_canonical_string(&d).unwrap(), before, "a failed patch left changes behind");
}

#[test]
fn ops_within_one_patch_can_build_on_each_other() {
    let mut d = doc();
    apply_ops(
        &mut d,
        &[
            Op::NodeInsert {
                parent: NodeId::from_static("nd_group"),
                index: None,
                node: frame("nd_wrap"),
            },
            Op::NodeMove {
                id: NodeId::from_static("nd_a"),
                parent: NodeId::from_static("nd_wrap"),
                index: None,
            },
        ],
    )
    .unwrap();

    assert_eq!(d.parent_of(&NodeId::from_static("nd_a")).unwrap().id.as_str(), "nd_wrap");
}

#[test]
fn ops_round_trip_through_json_the_way_mcp_will_send_them() {
    let ops = vec![
        Op::NodeInsert {
            parent: NodeId::from_static("nd_group"),
            index: Some(1),
            node: rect("nd_new", 4.0, 4.0),
        },
        Op::NodeUpdate {
            id: NodeId::from_static("nd_a"),
            path: "fills.0.color".into(),
            value: json!("#ff0055"),
        },
        Op::PathBoolean {
            ids: vec![NodeId::from_static("nd_a"), NodeId::from_static("nd_b")],
            mode: BoolOp::Subtract,
            result_id: None,
        },
    ];

    let wire = serde_json::to_string(&ops).unwrap();
    assert!(wire.contains("\"op\":\"node.insert\""), "got {wire}");
    assert!(wire.contains("\"op\":\"path.boolean\""), "got {wire}");

    let back: Vec<Op> = serde_json::from_str(&wire).unwrap();
    assert_eq!(ops, back);
}

#[test]
fn a_patch_reports_which_nodes_it_touched() {
    let mut d = doc();
    let (_, report) = apply_ops(
        &mut d,
        &[
            Op::NodeUpdate {
                id: NodeId::from_static("nd_a"),
                path: "width".into(),
                value: json!(1.0),
            },
            Op::NodeDelete { id: NodeId::from_static("nd_b") },
        ],
    )
    .unwrap();

    assert_eq!(report.applied, 2);
    assert!(report.touched.contains(&NodeId::from_static("nd_a")));
    assert!(report.touched.contains(&NodeId::from_static("nd_b")));
}

#[test]
fn the_session_wraps_all_of_this_in_one_undoable_step() {
    let mut s = Session::new(doc());
    let before = to_canonical_string(s.document()).unwrap();

    s.apply(
        "Build a card",
        vec![
            Op::NodeInsert {
                parent: NodeId::from_static("nd_group"),
                index: None,
                node: frame("nd_card"),
            },
            Op::NodeMove {
                id: NodeId::from_static("nd_a"),
                parent: NodeId::from_static("nd_card"),
                index: None,
            },
            Op::NodeUpdate {
                id: NodeId::from_static("nd_a"),
                path: "width".into(),
                value: json!(300.0),
            },
        ],
    )
    .unwrap();

    assert_eq!(s.history().depth().0, 1, "three ops should be one undo step");
    s.undo().unwrap();
    assert_eq!(to_canonical_string(s.document()).unwrap(), before);
}
