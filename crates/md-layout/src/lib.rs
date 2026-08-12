//! # md-layout
//!
//! Turning a design into a page that reflows.
//!
//! Two fields have existed in the document model since the beginning and did nothing:
//! [`md_doc::node::Constraints`] on every node, and [`md_doc::node::Layout`] on frames.
//! Without them an exported design is one picture that scales — on a phone a 1440-wide
//! layout renders at 27%, and 16px body text arrives at four pixels tall. That is the
//! single largest difference between "a graphic" and "a website", and this crate is what
//! closes it.
//!
//! ## What it does
//!
//! [`solve`] takes a page and a target width and returns a new page laid out for it.
//! Nothing is mutated: the design is the source, and each breakpoint is a *derivation*
//! from it, so a document has one truth rather than one per screen size.
//!
//! Two mechanisms, applied together, top down:
//!
//! **Constraints** answer "when my parent resizes, what happens to me?" — pin left, pin
//! right, centre, stretch both edges, or scale proportionally. This is Figma's model, and
//! it is the right one for absolutely positioned artwork, which most graphic design is.
//!
//! **Auto-layout** answers "where do my children go?" — a frame stacks them along an
//! axis with a gap and padding, aligns them across it, and can wrap. This is flexbox, and
//! it is the right model for the parts of a page that are a list of things.
//!
//! ## Text is why this needs real metrics
//!
//! Narrowing a text box changes how it wraps, which changes its height, which moves
//! everything below it in a stack. A layout engine that could not measure text could not
//! do that — it would have to assume text never changes height, and every column of copy
//! would overlap what follows it. This is the payoff of `md-text` doing the shaping.

use md_doc::node::{HConstraint, Node, NodeKind, VConstraint};
use md_doc::{Page, Transform};

mod stack;

/// Lay a page out at a target width.
///
/// The page's own width becomes `target_width`; its height becomes whatever the content
/// needs, because a narrower page is a taller one and cropping the overflow would lose
/// the design rather than reflow it.
pub fn solve(page: &Page, target_width: f64) -> Page {
    let mut out = page.clone();
    let target_width = target_width.max(1.0);

    let from = page.width.max(1.0);
    resize_frame(&mut out.root, target_width, page.height, from, page.height);

    out.width = target_width;

    // A page is at least as tall as the design on it. An auto-layout root states its own
    // height outright; otherwise the artboard's height is the designer's intent — the
    // whitespace at the bottom of a hero is a decision — so it only ever grows, and only
    // when something has reflowed past it.
    let stated = out.root.box_size().map(|(_, h)| h).unwrap_or(page.height);
    let content = content_bottom(&out.root);
    let height = if root_hugs(&out.root) {
        stated
    } else {
        stated.max(content)
    };

    out.height = height.max(1.0);
    out.root.set_box_size(target_width, out.height);
    out
}

/// Does the root decide its own height, rather than inheriting the artboard's?
fn root_hugs(root: &Node) -> bool {
    matches!(&root.kind, NodeKind::Frame(f) if f.layout.is_some())
}

/// The lowest edge any child reaches.
fn content_bottom(frame: &Node) -> f64 {
    frame
        .children
        .iter()
        .filter(|c| c.visible)
        .filter_map(|c| c.bounds_in_parent())
        .map(|b| b.y + b.h)
        .fold(0.0, f64::max)
}

/// Is this page's layout affected by width at all?
///
/// A design with no constraints and no auto-layout produces the same picture at every
/// width, and emitting three identical copies of it would triple a page's size to say
/// nothing. Callers use this to skip the work.
pub fn is_responsive(page: &Page) -> bool {
    fn walk(node: &Node) -> bool {
        if let NodeKind::Frame(f) = &node.kind {
            if f.layout.is_some() {
                return true;
            }
        }
        node.children
            .iter()
            .any(|c| !c.constraints.is_default() || walk(c))
    }
    walk(&page.root)
}

/// Resize a frame and everything inside it.
fn resize_frame(frame: &mut Node, new_w: f64, new_h: f64, old_w: f64, old_h: f64) {
    let auto = match &frame.kind {
        NodeKind::Frame(f) => f.layout.clone(),
        _ => None,
    };

    frame.set_box_size(new_w, new_h);

    match auto {
        // Auto-layout owns its children's positions outright, so constraints do not apply
        // inside one — the same rule Figma uses, and for the same reason: two systems
        // both claiming to place a child can only disagree.
        Some(layout) => {
            let content_height = stack::lay_out(frame, &layout, new_w);
            // A stack grows to fit what is in it. Without this a column of text that
            // wrapped to more lines at a narrower width would spill out of its own frame.
            if let NodeKind::Frame(f) = &mut frame.kind {
                f.height = content_height.max(0.0);
            }
        }
        None => {
            for child in &mut frame.children {
                apply_constraints(child, new_w, new_h, old_w, old_h);
            }
        }
    }
}

/// Reposition and resize one child for a parent that changed size.
fn apply_constraints(child: &mut Node, new_w: f64, new_h: f64, old_w: f64, old_h: f64) {
    let (x, y) = position(child);
    let size = child.box_size();

    // A group or a path has no box of its own, so "stretch" cannot mean anything for it.
    // Falling back to its bounds would let a stroke's overhang creep into the geometry,
    // so those kinds only ever move.
    let (w, h) = size.unwrap_or((0.0, 0.0));

    let (nx, nw) = solve_axis(
        child.constraints.h.into(),
        x,
        w,
        old_w,
        new_w,
        size.is_some(),
    );
    let (ny, nh) = solve_axis(
        child.constraints.v.into(),
        y,
        h,
        old_h,
        new_h,
        size.is_some(),
    );

    move_to(child, nx, ny);

    if size.is_some() && (nw != w || nh != h) {
        let (before_w, before_h) = (w, h);
        // A frame passes the change on to its own children before its size is final,
        // because an auto-layout frame's height is decided by what is inside it.
        if matches!(child.kind, NodeKind::Frame(_)) {
            resize_frame(child, nw, nh, before_w, before_h);
        } else {
            child.set_box_size(nw, nh);
        }
    } else if matches!(child.kind, NodeKind::Frame(_)) {
        // Same size, but an auto-layout child may still need to re-stack: the text inside
        // it can have re-wrapped even though the frame did not move.
        resize_frame(child, w, h, w, h);
    }
}

/// One axis of a constraint solve, shared by both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pin {
    Start,
    End,
    Center,
    Stretch,
    Scale,
}

impl From<HConstraint> for Pin {
    fn from(c: HConstraint) -> Self {
        match c {
            HConstraint::Left => Pin::Start,
            HConstraint::Right => Pin::End,
            HConstraint::Center => Pin::Center,
            HConstraint::Stretch => Pin::Stretch,
            HConstraint::Scale => Pin::Scale,
        }
    }
}

impl From<VConstraint> for Pin {
    fn from(c: VConstraint) -> Self {
        match c {
            VConstraint::Top => Pin::Start,
            VConstraint::Bottom => Pin::End,
            VConstraint::Center => Pin::Center,
            VConstraint::Stretch => Pin::Stretch,
            VConstraint::Scale => Pin::Scale,
        }
    }
}

/// Returns the new offset and size along one axis.
fn solve_axis(
    pin: Pin,
    offset: f64,
    size: f64,
    old_parent: f64,
    new_parent: f64,
    resizable: bool,
) -> (f64, f64) {
    let delta = new_parent - old_parent;
    let ratio = if old_parent.abs() > f64::EPSILON {
        new_parent / old_parent
    } else {
        1.0
    };

    match pin {
        Pin::Start => (offset, size),
        Pin::End => (offset + delta, size),
        // The *centre* stays proportionally placed, not the leading edge — otherwise a
        // centred element drifts as its parent grows.
        Pin::Center => (offset + delta / 2.0, size),
        Pin::Stretch if resizable => {
            // Both margins are held, so all the change lands in the size.
            (offset, size + delta)
        }
        // Nothing to stretch: hold the leading margin and let it sit where it is.
        Pin::Stretch => (offset, size),
        Pin::Scale => (offset * ratio, if resizable { size * ratio } else { size }),
    }
}

/// A node's offset within its parent, from its transform's translation.
fn position(node: &Node) -> (f64, f64) {
    let m = node.transform.0;
    (m[4], m[5])
}

/// Move a node, leaving any rotation or scale in its transform intact.
fn move_to(node: &mut Node, x: f64, y: f64) {
    node.transform = Transform([
        node.transform.0[0],
        node.transform.0[1],
        node.transform.0[2],
        node.transform.0[3],
        x,
        y,
    ]);
}

/// The height a node occupies in a stack, measured rather than assumed.
fn outer_height(node: &Node) -> f64 {
    node.box_size()
        .map(|(_, h)| h)
        .or_else(|| node.local_bounds().map(|b| b.h))
        .unwrap_or(0.0)
}

fn outer_width(node: &Node) -> f64 {
    node.box_size()
        .map(|(w, _)| w)
        .or_else(|| node.local_bounds().map(|b| b.w))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::node::{
        AlignItems, Constraints, Direction, FrameGeometry, JustifyContent, Layout, RectGeometry,
    };
    use md_doc::text::TextGeometry;
    use md_doc::{NodeId, PageId};

    fn page(width: f64, height: f64) -> Page {
        let mut p = Page::new(PageId::from_static("pg_1"), "Home", "index", width, height);
        if let NodeKind::Frame(f) = &mut p.root.kind {
            f.width = width;
            f.height = height;
        }
        p
    }

    fn rect(id: &str, x: f64, y: f64, w: f64, h: f64) -> Node {
        Node::new(
            NodeId::parse(id).unwrap(),
            NodeKind::Rect(RectGeometry {
                width: w,
                height: h,
                corner_radius: [0.0; 4],
            }),
        )
        .with_transform(Transform::translate(x, y))
    }

    fn find<'a>(p: &'a Page, id: &str) -> &'a Node {
        p.root.find(&NodeId::parse(id).unwrap()).expect(id)
    }

    fn geometry(p: &Page, id: &str) -> (f64, f64, f64, f64) {
        let node = find(p, id);
        let (x, y) = position(node);
        let (w, h) = node.box_size().unwrap_or((0.0, 0.0));
        (x, y, w, h)
    }

    // -----------------------------------------------------------------------
    // Constraints
    // -----------------------------------------------------------------------

    #[test]
    fn a_left_pinned_child_does_not_move() {
        let mut p = page(1000.0, 600.0);
        p.root.children.push(rect("nd_a", 40.0, 40.0, 200.0, 100.0));

        let solved = solve(&p, 500.0);
        assert_eq!(geometry(&solved, "nd_a"), (40.0, 40.0, 200.0, 100.0));
    }

    #[test]
    fn a_right_pinned_child_keeps_its_right_margin() {
        let mut p = page(1000.0, 600.0);
        let mut node = rect("nd_a", 760.0, 40.0, 200.0, 100.0); // 40 from the right edge
        node.constraints = Constraints {
            h: HConstraint::Right,
            v: VConstraint::Top,
        };
        p.root.children.push(node);

        let solved = solve(&p, 600.0);
        let (x, _, w, _) = geometry(&solved, "nd_a");
        assert_eq!(600.0 - (x + w), 40.0, "the right margin changed");
    }

    #[test]
    fn a_centred_child_stays_centred() {
        let mut p = page(1000.0, 600.0);
        let mut node = rect("nd_a", 400.0, 40.0, 200.0, 100.0);
        node.constraints = Constraints {
            h: HConstraint::Center,
            v: VConstraint::Top,
        };
        p.root.children.push(node);

        let solved = solve(&p, 600.0);
        let (x, _, w, _) = geometry(&solved, "nd_a");
        assert!(
            (x + w / 2.0 - 300.0).abs() < 1e-9,
            "centre landed at {}",
            x + w / 2.0
        );
    }

    #[test]
    fn a_stretched_child_holds_both_margins() {
        let mut p = page(1000.0, 600.0);
        let mut node = rect("nd_a", 60.0, 40.0, 880.0, 100.0); // 60 each side
        node.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(node);

        let solved = solve(&p, 400.0);
        let (x, _, w, _) = geometry(&solved, "nd_a");
        assert_eq!(x, 60.0);
        assert_eq!(400.0 - (x + w), 60.0, "the right margin changed");
    }

    #[test]
    fn a_scaled_child_keeps_its_proportions() {
        let mut p = page(1000.0, 600.0);
        let mut node = rect("nd_a", 100.0, 40.0, 200.0, 100.0);
        node.constraints = Constraints {
            h: HConstraint::Scale,
            v: VConstraint::Top,
        };
        p.root.children.push(node);

        let solved = solve(&p, 500.0);
        let (x, _, w, _) = geometry(&solved, "nd_a");
        assert_eq!((x, w), (50.0, 100.0));
    }

    #[test]
    fn a_stretched_group_moves_rather_than_stretching() {
        // A group has no box, so there is nothing to stretch. Silently doing nothing is
        // better than resizing it through its transform, which would scale its strokes.
        let mut p = page(1000.0, 600.0);
        let mut group = Node::new(NodeId::from_static("nd_g"), NodeKind::Group)
            .with_transform(Transform::translate(100.0, 100.0));
        group.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        group.children.push(rect("nd_in", 0.0, 0.0, 50.0, 50.0));
        p.root.children.push(group);

        let solved = solve(&p, 500.0);
        assert_eq!(position(find(&solved, "nd_g")), (100.0, 100.0));
        assert_eq!(geometry(&solved, "nd_in").2, 50.0);
    }

    #[test]
    fn constraints_reach_all_the_way_down() {
        let mut p = page(1000.0, 600.0);

        let mut inner = rect("nd_in", 20.0, 20.0, 360.0, 60.0);
        inner.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };

        let mut outer = Node::new(
            NodeId::from_static("nd_frame"),
            NodeKind::Frame(FrameGeometry {
                width: 400.0,
                height: 200.0,
                clip: false,
                corner_radius: [0.0; 4],
                layout: None,
            }),
        )
        .with_transform(Transform::translate(0.0, 0.0));
        outer.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        outer.children.push(inner);
        p.root.children.push(outer);

        // Page 1000 → 900 is −100. The stretched frame absorbs it (400 → 300), and its
        // stretched child absorbs the same change again (360 → 260), keeping its 20pt
        // margins on both sides.
        let solved = solve(&p, 900.0);
        assert_eq!(geometry(&solved, "nd_frame").2, 300.0);
        let (x, _, w, _) = geometry(&solved, "nd_in");
        assert_eq!((x, w), (20.0, 260.0));
    }

    // -----------------------------------------------------------------------
    // Auto-layout
    // -----------------------------------------------------------------------

    fn stack_frame(id: &str, w: f64, h: f64, layout: Layout) -> Node {
        Node::new(
            NodeId::parse(id).unwrap(),
            NodeKind::Frame(FrameGeometry {
                width: w,
                height: h,
                clip: false,
                corner_radius: [0.0; 4],
                layout: Some(layout),
            }),
        )
    }

    #[test]
    fn a_vertical_stack_places_its_children_in_order() {
        let mut p = page(600.0, 800.0);
        let mut frame = stack_frame(
            "nd_stack",
            600.0,
            0.0,
            Layout {
                direction: Direction::Vertical,
                gap: 20.0,
                padding: [30.0, 10.0, 30.0, 10.0],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: false,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 100.0, 50.0));
        frame.children.push(rect("nd_b", 0.0, 0.0, 100.0, 70.0));
        p.root.children.push(frame);

        let solved = solve(&p, 600.0);
        assert_eq!(geometry(&solved, "nd_a"), (10.0, 30.0, 100.0, 50.0));
        assert_eq!(geometry(&solved, "nd_b"), (10.0, 100.0, 100.0, 70.0));
        // 30 top + 50 + 20 gap + 70 + 30 bottom
        assert_eq!(geometry(&solved, "nd_stack").3, 200.0);
    }

    #[test]
    fn a_horizontal_stack_runs_across() {
        let mut p = page(600.0, 400.0);
        let mut frame = stack_frame(
            "nd_row",
            600.0,
            0.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 16.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: false,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 100.0, 40.0));
        frame.children.push(rect("nd_b", 0.0, 0.0, 100.0, 40.0));
        p.root.children.push(frame);

        let solved = solve(&p, 600.0);
        assert_eq!(position(find(&solved, "nd_a")), (0.0, 0.0));
        assert_eq!(position(find(&solved, "nd_b")), (116.0, 0.0));
    }

    #[test]
    fn a_row_wraps_when_it_runs_out_of_width() {
        let mut p = page(1000.0, 400.0);
        let mut frame = stack_frame(
            "nd_row",
            1000.0,
            0.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 20.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: true,
            },
        );
        for id in ["nd_a", "nd_b", "nd_c"] {
            frame.children.push(rect(id, 0.0, 0.0, 300.0, 100.0));
        }
        // A frame is only responsive if it is told to be — the default is a fixed box
        // pinned to the top left, which is what a piece of artwork wants.
        frame.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(frame);

        // 3 × 300 + 2 × 20 = 940, fits on one line at 1000.
        let wide = solve(&p, 1000.0);
        assert_eq!(position(find(&wide, "nd_c")).1, 0.0, "wrapped too early");

        // At 700 only two fit, so the third drops to a second line.
        let narrow = solve(&p, 700.0);
        assert_eq!(
            position(find(&narrow, "nd_c")).1,
            120.0,
            "the row did not wrap"
        );
        assert_eq!(geometry(&narrow, "nd_row").3, 220.0);
    }

    #[test]
    fn stretch_alignment_fills_the_cross_axis() {
        let mut p = page(600.0, 400.0);
        let mut frame = stack_frame(
            "nd_stack",
            600.0,
            0.0,
            Layout {
                direction: Direction::Vertical,
                gap: 0.0,
                padding: [0.0, 40.0, 0.0, 40.0],
                align: AlignItems::Stretch,
                justify: JustifyContent::Start,
                wrap: false,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 100.0, 50.0));
        p.root.children.push(frame);

        let solved = solve(&p, 600.0);
        // 600 − 40 − 40 of padding.
        assert_eq!(geometry(&solved, "nd_a"), (40.0, 0.0, 520.0, 50.0));
    }

    #[test]
    fn centre_alignment_centres_across_the_stack() {
        let mut p = page(600.0, 400.0);
        let mut frame = stack_frame(
            "nd_stack",
            600.0,
            0.0,
            Layout {
                direction: Direction::Vertical,
                gap: 0.0,
                padding: [0.0; 4],
                align: AlignItems::Center,
                justify: JustifyContent::Start,
                wrap: false,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 200.0, 50.0));
        p.root.children.push(frame);

        assert_eq!(position(find(&solve(&p, 600.0), "nd_a")).0, 200.0);
    }

    #[test]
    fn space_between_pushes_the_ends_apart() {
        let mut p = page(600.0, 400.0);
        let mut frame = stack_frame(
            "nd_row",
            600.0,
            100.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 0.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::SpaceBetween,
                wrap: false,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 100.0, 40.0));
        frame.children.push(rect("nd_b", 0.0, 0.0, 100.0, 40.0));
        frame.children.push(rect("nd_c", 0.0, 0.0, 100.0, 40.0));
        p.root.children.push(frame);

        let solved = solve(&p, 600.0);
        assert_eq!(position(find(&solved, "nd_a")).0, 0.0);
        assert_eq!(position(find(&solved, "nd_b")).0, 250.0);
        assert_eq!(position(find(&solved, "nd_c")).0, 500.0);
    }

    #[test]
    fn a_single_child_with_space_between_stays_at_the_start() {
        // The degenerate case CSS also has to define: with one item there is no space to
        // put between anything.
        let mut p = page(600.0, 400.0);
        let mut frame = stack_frame(
            "nd_row",
            600.0,
            100.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 0.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::SpaceBetween,
                wrap: false,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 100.0, 40.0));
        p.root.children.push(frame);

        assert_eq!(position(find(&solve(&p, 600.0), "nd_a")).0, 0.0);
    }

    #[test]
    fn a_child_too_wide_for_its_stack_is_narrowed_rather_than_left_hanging_out() {
        // CSS shrinks by default, and this is the case that matters: a card designed at
        // 400 in a row that is 294 wide on a phone.
        let mut p = page(1440.0, 900.0);
        let mut frame = stack_frame(
            "nd_row",
            1440.0,
            0.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 24.0,
                padding: [0.0, 48.0, 0.0, 48.0],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: true,
            },
        );
        for id in ["nd_a", "nd_b"] {
            frame.children.push(rect(id, 0.0, 0.0, 400.0, 220.0));
        }
        frame.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(frame);

        let phone = solve(&p, 390.0);
        let (x, _, w, _) = geometry(&phone, "nd_a");
        assert_eq!(w, 294.0, "the card kept a width that does not fit");
        assert!(
            x + w <= 390.0 - 48.0 + 0.001,
            "the card still hangs past the padding"
        );
    }

    #[test]
    fn a_child_that_already_fits_is_left_alone() {
        let mut p = page(1000.0, 600.0);
        let mut frame = stack_frame(
            "nd_row",
            1000.0,
            0.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 0.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: true,
            },
        );
        frame.children.push(rect("nd_a", 0.0, 0.0, 200.0, 100.0));
        p.root.children.push(frame);

        assert_eq!(geometry(&solve(&p, 1000.0), "nd_a").2, 200.0);
    }

    #[test]
    fn a_hidden_child_takes_up_no_space() {
        let mut p = page(600.0, 400.0);
        let mut frame = stack_frame(
            "nd_stack",
            600.0,
            0.0,
            Layout {
                direction: Direction::Vertical,
                gap: 10.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: false,
            },
        );
        let mut hidden = rect("nd_hidden", 0.0, 0.0, 100.0, 100.0);
        hidden.visible = false;
        frame.children.push(hidden);
        frame.children.push(rect("nd_a", 0.0, 0.0, 100.0, 50.0));
        p.root.children.push(frame);

        let solved = solve(&p, 600.0);
        assert_eq!(
            position(find(&solved, "nd_a")).1,
            0.0,
            "a hidden layer left a gap where it used to be"
        );
        assert_eq!(geometry(&solved, "nd_stack").3, 50.0);
    }

    // -----------------------------------------------------------------------
    // Where layout and text meet
    // -----------------------------------------------------------------------

    #[test]
    fn narrowing_a_stack_reflows_the_text_and_moves_what_follows() {
        // The reason a layout engine needs real metrics: text that wraps to more lines is
        // taller, and everything below it has to move down. This is the whole thing.
        let mut p = page(800.0, 600.0);
        let mut frame = stack_frame(
            "nd_stack",
            800.0,
            0.0,
            Layout {
                direction: Direction::Vertical,
                gap: 20.0,
                padding: [0.0; 4],
                align: AlignItems::Stretch,
                justify: JustifyContent::Start,
                wrap: false,
            },
        );

        let mut copy = TextGeometry::new(
            "A paragraph long enough that narrowing its measure forces it onto more lines \
             than it needed before, which is the case the whole engine exists to handle.",
            "Inter",
            18.0,
        );
        copy.width = Some(800.0);
        frame.children.push(Node::new(
            NodeId::from_static("nd_copy"),
            NodeKind::Text(copy),
        ));
        frame.children.push(rect("nd_below", 0.0, 0.0, 100.0, 40.0));
        frame.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(frame);

        let wide = solve(&p, 800.0);
        let narrow = solve(&p, 320.0);

        let wide_y = position(find(&wide, "nd_below")).1;
        let narrow_y = position(find(&narrow, "nd_below")).1;
        assert!(
            narrow_y > wide_y,
            "the text did not get taller when it got narrower: {wide_y} vs {narrow_y}"
        );
        assert!(
            geometry(&narrow, "nd_stack").3 > geometry(&wide, "nd_stack").3,
            "the stack did not grow to fit the reflowed text"
        );
    }

    // -----------------------------------------------------------------------
    // The page as a whole
    // -----------------------------------------------------------------------

    #[test]
    fn solving_never_touches_the_original() {
        let mut p = page(1000.0, 600.0);
        let mut node = rect("nd_a", 0.0, 0.0, 900.0, 100.0);
        node.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(node);

        let before = serde_json::to_string(&p).unwrap();
        let _ = solve(&p, 400.0);
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            before,
            "the design was mutated; each breakpoint must be a derivation, not an edit"
        );
    }

    #[test]
    fn a_design_with_nothing_responsive_in_it_says_so() {
        let mut p = page(1000.0, 600.0);
        p.root.children.push(rect("nd_a", 0.0, 0.0, 100.0, 100.0));
        assert!(
            !is_responsive(&p),
            "a fixed design should not be re-emitted per breakpoint"
        );

        let mut node = rect("nd_b", 0.0, 0.0, 100.0, 100.0);
        node.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(node);
        assert!(is_responsive(&p));
    }

    #[test]
    fn a_stack_anywhere_makes_a_page_responsive() {
        let mut p = page(1000.0, 600.0);
        p.root.children.push(stack_frame(
            "nd_s",
            100.0,
            100.0,
            Layout {
                direction: Direction::Vertical,
                gap: 0.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: false,
            },
        ));
        assert!(is_responsive(&p));
    }

    #[test]
    fn a_zero_or_negative_width_does_not_produce_nonsense() {
        let mut p = page(1000.0, 600.0);
        let mut node = rect("nd_a", 100.0, 0.0, 800.0, 100.0);
        node.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(node);

        for width in [0.0, -50.0, 1.0] {
            let solved = solve(&p, width);
            let (x, y, w, h) = geometry(&solved, "nd_a");
            for v in [x, y, w, h, solved.width, solved.height] {
                assert!(v.is_finite(), "non-finite geometry at width {width}");
            }
            assert!(w >= 0.0, "negative width at target {width}");
        }
    }

    #[test]
    fn the_page_grows_taller_as_it_narrows() {
        let mut p = page(800.0, 100.0);
        let mut frame = stack_frame(
            "nd_stack",
            800.0,
            0.0,
            Layout {
                direction: Direction::Horizontal,
                gap: 0.0,
                padding: [0.0; 4],
                align: AlignItems::Start,
                justify: JustifyContent::Start,
                wrap: true,
            },
        );
        for id in ["nd_a", "nd_b", "nd_c", "nd_d"] {
            frame.children.push(rect(id, 0.0, 0.0, 200.0, 100.0));
        }
        frame.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(frame);

        assert_eq!(solve(&p, 800.0).height, 100.0);
        assert_eq!(solve(&p, 400.0).height, 200.0);
        assert_eq!(solve(&p, 200.0).height, 400.0);
    }
}
