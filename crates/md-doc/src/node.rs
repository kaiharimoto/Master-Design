//! The scene graph.
//!
//! One `Node` type covers every kind of thing on a page, with the kind-specific data in
//! [`NodeKind`]. Traversal stays uniform — every node has `children`, even the ones that
//! are not allowed any — which keeps the patch protocol, the query engine and the
//! exporter from each needing their own special cases.
//!
//! Two fields do more work than their size suggests:
//!
//! * `roles` — semantic tags. Animations bind to roles rather than ids, which is what
//!   lets one animation package apply to any document, and what lets an instruction like
//!   "stagger the cards" resolve without anyone knowing an id.
//! * `a11y` — carried through to the export. A generated page that is beautiful and
//!   unusable with a screen reader is not finished, and retrofitting semantics onto
//!   flat vector output afterwards is close to impossible.

use crate::id::NodeId;
use crate::paint::{is_false, BlendMode, Effect, Paint, Stroke};
use crate::text::TextGeometry;
use crate::transform::Transform;
use md_geom::{Bounds, FillRule};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: NodeId,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,

    #[serde(flatten)]
    pub kind: NodeKind,

    #[serde(default, skip_serializing_if = "Transform::is_identity")]
    pub transform: Transform,

    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub visible: bool,

    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,

    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,

    #[serde(default, skip_serializing_if = "is_default_blend")]
    pub blend_mode: BlendMode,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fills: Vec<Paint>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<Stroke>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,

    /// Semantic tags. Lowercase kebab-case by convention: `hero-title`, `card`, `cta`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a11y: Option<A11y>,

    #[serde(default, skip_serializing_if = "Constraints::is_default")]
    pub constraints: Constraints,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NodeKind {
    /// A box that lays out and optionally clips its children. Sections, cards,
    /// artboards — all frames.
    Frame(FrameGeometry),
    /// Children grouped for selection and transform, with no box of its own.
    Group,
    Path(PathGeometry),
    Rect(RectGeometry),
    Ellipse(EllipseGeometry),
    Text(TextGeometry),
    Image(ImageGeometry),
}

impl NodeKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            NodeKind::Frame(_) => "frame",
            NodeKind::Group => "group",
            NodeKind::Path(_) => "path",
            NodeKind::Rect(_) => "rect",
            NodeKind::Ellipse(_) => "ellipse",
            NodeKind::Text(_) => "text",
            NodeKind::Image(_) => "image",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameGeometry {
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clip: bool,
    #[serde(default, skip_serializing_if = "is_no_radius")]
    pub corner_radius: [f64; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathGeometry {
    /// SVG path data, normalized to absolute cubics by the geometry kernel.
    pub d: String,
    #[serde(default, skip_serializing_if = "is_default_fill_rule")]
    pub fill_rule: FillRule,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RectGeometry {
    pub width: f64,
    pub height: f64,
    /// Top-left, top-right, bottom-right, bottom-left — CSS order.
    #[serde(default, skip_serializing_if = "is_no_radius")]
    pub corner_radius: [f64; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EllipseGeometry {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGeometry {
    /// Path relative to the project's `assets/` directory.
    pub asset: String,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "is_default_fit")]
    pub fit: crate::paint::ImageFit,
}

/// Accessibility metadata, carried into the exported markup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct A11y {
    /// ARIA role, or a semantic element name like `heading` or `button`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 1..6, for headings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_level: Option<u8>,
    /// Alternative text for images and meaningful graphics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    /// Decorative: hide from assistive technology.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

/// How a node reacts when its parent frame resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Constraints {
    #[serde(default)]
    pub h: HConstraint,
    #[serde(default)]
    pub v: VConstraint,
}

impl Constraints {
    pub fn is_default(&self) -> bool {
        *self == Constraints::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HConstraint {
    #[default]
    Left,
    Right,
    Center,
    /// Pin both edges.
    Stretch,
    /// Keep proportional position and size.
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VConstraint {
    #[default]
    Top,
    Bottom,
    Center,
    Stretch,
    Scale,
}

/// Auto-layout on a frame. `None` means children are positioned absolutely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gap: f64,
    /// Top, right, bottom, left — CSS order.
    #[serde(default, skip_serializing_if = "is_no_radius")]
    pub padding: [f64; 4],
    #[serde(default)]
    pub align: AlignItems,
    #[serde(default)]
    pub justify: JustifyContent,
    #[serde(default, skip_serializing_if = "is_false")]
    pub wrap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AlignItems {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum JustifyContent {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

// ---------------------------------------------------------------------------
// Construction and queries
// ---------------------------------------------------------------------------

impl Node {
    pub fn new(id: NodeId, kind: NodeKind) -> Self {
        Node {
            id,
            name: String::new(),
            kind,
            transform: Transform::IDENTITY,
            visible: true,
            locked: false,
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            fills: Vec::new(),
            strokes: Vec::new(),
            effects: Vec::new(),
            roles: Vec::new(),
            a11y: None,
            constraints: Constraints::default(),
            children: Vec::new(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.roles.push(role.into());
        self
    }

    pub fn with_fill(mut self, paint: Paint) -> Self {
        self.fills.push(paint);
        self
    }

    pub fn with_transform(mut self, t: Transform) -> Self {
        self.transform = t;
        self
    }

    pub fn with_children(mut self, children: Vec<Node>) -> Self {
        self.children = children;
        self
    }

    /// Can this kind legally hold children?
    pub fn is_container(&self) -> bool {
        matches!(self.kind, NodeKind::Frame(_) | NodeKind::Group)
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Display name, falling back to the type when the node was never named.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.kind.type_name().to_string()
        } else {
            self.name.clone()
        }
    }

    /// Resolve this node's geometry to SVG path data in its own coordinate space.
    ///
    /// Live shapes are baked here rather than in the document, so a rectangle stays an
    /// editable rectangle with a corner-radius slider right up until something actually
    /// needs its outline.
    pub fn geometry_path(&self) -> Option<String> {
        match &self.kind {
            NodeKind::Path(p) => Some(p.d.clone()),
            NodeKind::Rect(r) => Some(md_geom::rect_path(
                0.0,
                0.0,
                r.width,
                r.height,
                r.corner_radius,
            )),
            NodeKind::Ellipse(e) => Some(md_geom::ellipse_path(
                e.width / 2.0,
                e.height / 2.0,
                e.width / 2.0,
                e.height / 2.0,
            )),
            NodeKind::Frame(f) => Some(md_geom::rect_path(
                0.0,
                0.0,
                f.width,
                f.height,
                f.corner_radius,
            )),
            NodeKind::Image(i) => Some(md_geom::rect_path(0.0, 0.0, i.width, i.height, [0.0; 4])),
            NodeKind::Text(_) | NodeKind::Group => None,
        }
    }

    /// Bounding box in this node's own coordinate space, before its transform.
    ///
    /// Text is estimated from font metrics we do not have here — the returned box is a
    /// usable approximation for layout and selection, not a typesetting result. The
    /// renderer measures for real.
    pub fn local_bounds(&self) -> Option<Bounds> {
        match &self.kind {
            NodeKind::Text(t) => {
                let lines = t.hard_lines();
                let height = t.line_height_units() * lines.len().max(1) as f64;
                let width = t.width.unwrap_or_else(|| {
                    // Rough advance width: most Latin text averages a little over half
                    // an em per character.
                    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
                    longest as f64 * t.font.font_size * 0.55
                });
                Some(Bounds {
                    x: 0.0,
                    y: 0.0,
                    w: width,
                    h: t.height.unwrap_or(height),
                })
            }
            NodeKind::Group => {
                let mut acc: Option<Bounds> = None;
                for child in &self.children {
                    if let Some(b) = child.bounds_in_parent() {
                        acc = Some(match acc {
                            Some(a) => a.union(&b),
                            None => b,
                        });
                    }
                }
                acc
            }
            _ => self.geometry_path().and_then(|d| md_geom::bounds(&d).ok()),
        }
    }

    /// Bounding box after applying this node's own transform — the box its parent sees.
    pub fn bounds_in_parent(&self) -> Option<Bounds> {
        let b = self.local_bounds()?;
        if self.transform.is_identity() {
            return Some(b);
        }
        // Transform all four corners; a rotated box's axis-aligned extent is not the
        // transform of its extent.
        let corners = [
            self.transform.apply(b.x, b.y),
            self.transform.apply(b.x + b.w, b.y),
            self.transform.apply(b.x + b.w, b.y + b.h),
            self.transform.apply(b.x, b.y + b.h),
        ];
        let xs = corners.iter().map(|c| c[0]);
        let ys = corners.iter().map(|c| c[1]);
        let x0 = xs.clone().fold(f64::INFINITY, f64::min);
        let x1 = xs.fold(f64::NEG_INFINITY, f64::max);
        let y0 = ys.clone().fold(f64::INFINITY, f64::min);
        let y1 = ys.fold(f64::NEG_INFINITY, f64::max);
        Some(Bounds {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    /// Depth-first walk over this node and its descendants.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }

    pub fn walk_mut(&mut self, f: &mut impl FnMut(&mut Node)) {
        f(self);
        for c in &mut self.children {
            c.walk_mut(f);
        }
    }

    pub fn find(&self, id: &NodeId) -> Option<&Node> {
        if &self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    pub fn find_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        if &self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// Every id in this subtree, including the node's own.
    pub fn all_ids(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.walk(&mut |n| out.push(n.id.clone()));
        out
    }

    pub fn descendant_count(&self) -> usize {
        let mut n = 0;
        self.walk(&mut |_| n += 1);
        n - 1
    }
}

// ---------------------------------------------------------------------------
// serde helpers
// ---------------------------------------------------------------------------

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
fn is_no_radius(v: &[f64; 4]) -> bool {
    v.iter().all(|r| r.abs() < f64::EPSILON)
}
fn is_default_blend(v: &BlendMode) -> bool {
    *v == BlendMode::Normal
}
fn is_default_fill_rule(v: &FillRule) -> bool {
    *v == FillRule::default()
}
fn is_default_fit(v: &crate::paint::ImageFit) -> bool {
    *v == crate::paint::ImageFit::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f64, h: f64) -> Node {
        Node::new(
            NodeId::from_static("nd_r"),
            NodeKind::Rect(RectGeometry {
                width: w,
                height: h,
                corner_radius: [0.0; 4],
            }),
        )
    }

    #[test]
    fn kind_is_flattened_with_a_type_tag() {
        let v = serde_json::to_value(rect(10.0, 20.0)).unwrap();
        assert_eq!(v["type"], "rect");
        assert_eq!(v["width"], 10.0);
        assert!(
            v.get("kind").is_none(),
            "kind should be flattened away: {v}"
        );
    }

    #[test]
    fn defaults_do_not_reach_the_file() {
        let json = serde_json::to_string(&rect(10.0, 20.0)).unwrap();
        for noisy in [
            "visible",
            "locked",
            "opacity",
            "blendMode",
            "transform",
            "children",
        ] {
            assert!(
                !json.contains(noisy),
                "{noisy} should have been skipped: {json}"
            );
        }
    }

    #[test]
    fn a_node_round_trips() {
        let mut n = rect(10.0, 20.0).with_name("Card").with_role("card");
        n.fills.push(Paint::solid("#ff0055").unwrap());
        n.transform = Transform::translate(4.0, 5.0);
        let json = serde_json::to_string(&n).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn only_frames_and_groups_accept_children() {
        assert!(!rect(1.0, 1.0).is_container());
        assert!(Node::new(NodeId::from_static("nd_g"), NodeKind::Group).is_container());
        assert!(Node::new(
            NodeId::from_static("nd_f"),
            NodeKind::Frame(FrameGeometry {
                width: 10.0,
                height: 10.0,
                clip: false,
                corner_radius: [0.0; 4],
                layout: None,
            })
        )
        .is_container());
    }

    #[test]
    fn live_shapes_bake_to_paths_on_demand() {
        let d = rect(10.0, 20.0).geometry_path().unwrap();
        assert_eq!(d, "M 0 0 L 10 0 L 10 20 L 0 20 Z");
    }

    #[test]
    fn ellipse_is_centred_in_its_box() {
        let n = Node::new(
            NodeId::from_static("nd_e"),
            NodeKind::Ellipse(EllipseGeometry {
                width: 40.0,
                height: 20.0,
            }),
        );
        let b = n.local_bounds().unwrap();
        assert!(
            (b.w - 40.0).abs() < 0.05 && (b.h - 20.0).abs() < 0.05,
            "got {b:?}"
        );
        assert!(b.x.abs() < 0.05 && b.y.abs() < 0.05, "got {b:?}");
    }

    #[test]
    fn rotating_a_box_grows_its_axis_aligned_extent() {
        let mut n = rect(10.0, 10.0);
        n.transform = Transform::rotate(std::f64::consts::FRAC_PI_4);
        let b = n.bounds_in_parent().unwrap();
        let diagonal = (200.0f64).sqrt();
        assert!((b.w - diagonal).abs() < 1e-6, "got {b:?}");
    }

    #[test]
    fn group_bounds_are_the_union_of_children() {
        let mut a = rect(10.0, 10.0);
        a.id = NodeId::from_static("nd_a");
        let mut b = rect(10.0, 10.0);
        b.id = NodeId::from_static("nd_b");
        b.transform = Transform::translate(50.0, 0.0);

        let g = Node::new(NodeId::from_static("nd_g"), NodeKind::Group).with_children(vec![a, b]);
        let bounds = g.local_bounds().unwrap();
        assert_eq!(bounds.x, 0.0);
        assert_eq!(bounds.w, 60.0);
    }

    #[test]
    fn find_reaches_into_the_subtree() {
        let mut child = rect(1.0, 1.0);
        child.id = NodeId::from_static("nd_deep");
        let g = Node::new(NodeId::from_static("nd_g"), NodeKind::Group).with_children(vec![child]);
        assert!(g.find(&NodeId::from_static("nd_deep")).is_some());
        assert!(g.find(&NodeId::from_static("nd_missing")).is_none());
        assert_eq!(g.descendant_count(), 1);
    }
}
