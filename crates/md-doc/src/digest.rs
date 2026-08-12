//! A compact rendering of a document, for a model to read.
//!
//! Handing an AI the raw project JSON works and is a waste: most of those bytes are
//! defaults, ids repeated in full, and structure the model has to reassemble in its
//! head. A digest of a real page runs an order of magnitude smaller than its JSON while
//! saying more of what matters — the tree shape, the roles available to address, what is
//! animated by what.
//!
//! The output is for reading, not parsing. Anything that needs exact values should use
//! `doc.query` and read the node.

use crate::anim::Timeline;
use crate::document::{Document, Page};
use crate::node::{Node, NodeKind};
use crate::paint::Paint;
use std::fmt::Write;

/// Knobs for how much to include.
#[derive(Debug, Clone)]
pub struct DigestOptions {
    /// How deep into the scene graph to descend. Deeper levels are summarized as a count.
    pub max_depth: usize,
    pub include_timelines: bool,
    /// Restrict to one page, by id or slug.
    pub page: Option<String>,
    /// Truncate text content to this many characters.
    pub text_preview: usize,
}

impl Default for DigestOptions {
    fn default() -> Self {
        DigestOptions {
            max_depth: 12,
            include_timelines: true,
            page: None,
            text_preview: 60,
        }
    }
}

/// Render the whole document.
pub fn digest(doc: &Document, opts: &DigestOptions) -> String {
    let mut out = String::new();

    let _ = writeln!(
        out,
        "document \"{}\" — {} page{}, {} nodes, {} timelines",
        doc.meta.name,
        doc.pages.len(),
        if doc.pages.len() == 1 { "" } else { "s" },
        doc.node_count(),
        doc.timeline_count()
    );

    if !doc.meta.description.is_empty() {
        let _ = writeln!(out, "  {}", doc.meta.description);
    }

    if !doc.tokens.is_empty() {
        let mut parts = Vec::new();
        if !doc.tokens.colors.is_empty() {
            parts.push(format!(
                "colors: {}",
                doc.tokens
                    .colors
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        if !doc.tokens.fonts.is_empty() {
            parts.push(format!(
                "fonts: {}",
                doc.tokens.fonts.keys().cloned().collect::<Vec<_>>().join(" ")
            ));
        }
        if !doc.tokens.spacing.is_empty() {
            parts.push(format!(
                "spacing: {}",
                doc.tokens.spacing.keys().cloned().collect::<Vec<_>>().join(" ")
            ));
        }
        for p in parts {
            let _ = writeln!(out, "  {p}");
        }
    }

    // The role vocabulary is the first thing worth knowing: it is what selectors,
    // animations and instructions can address.
    let roles = doc.role_index();
    if !roles.is_empty() {
        let listed = roles
            .iter()
            .map(|(r, c)| if *c == 1 { format!("@{r}") } else { format!("@{r}×{c}") })
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "  roles: {listed}");
    }

    for page in &doc.pages {
        if let Some(filter) = &opts.page {
            if page.id.as_str() != filter && page.slug != *filter {
                continue;
            }
        }
        out.push('\n');
        digest_page(&mut out, page, opts);
    }

    out
}

fn digest_page(out: &mut String, page: &Page, opts: &DigestOptions) {
    let bg = page
        .background
        .as_ref()
        .map(|p| format!("  bg:{}", paint_summary(p)))
        .unwrap_or_default();

    let _ = writeln!(
        out,
        "page \"{}\" /{}  {}×{}{}",
        page.name,
        page.slug,
        num(page.width),
        num(page.height),
        bg
    );

    write_node(out, &page.root, 1, opts);

    if opts.include_timelines && !page.timelines.is_empty() {
        let _ = writeln!(out, "  timelines:");
        for tl in &page.timelines {
            write_timeline(out, tl);
        }
    }
}

fn write_node(out: &mut String, node: &Node, depth: usize, opts: &DigestOptions) {
    let indent = "  ".repeat(depth);

    if depth > opts.max_depth {
        let hidden = node.descendant_count() + 1;
        let _ = writeln!(out, "{indent}… {hidden} more node(s) below this depth");
        return;
    }

    let mut line = format!("{indent}{} {}", node.kind.type_name(), node.id);

    if !node.name.is_empty() {
        let _ = write!(line, " \"{}\"", node.name);
    }
    for role in &node.roles {
        let _ = write!(line, " @{role}");
    }

    // Size, in whatever terms this kind of node has one.
    match &node.kind {
        NodeKind::Frame(f) => {
            let _ = write!(line, " {}×{}", num(f.width), num(f.height));
            if f.layout.is_some() {
                let _ = write!(line, " auto-layout");
            }
        }
        NodeKind::Rect(r) => {
            let _ = write!(line, " {}×{}", num(r.width), num(r.height));
            if r.corner_radius.iter().any(|v| *v > 0.0) {
                let _ = write!(line, " r:{}", num(r.corner_radius[0]));
            }
        }
        NodeKind::Ellipse(e) => {
            let _ = write!(line, " {}×{}", num(e.width), num(e.height));
        }
        NodeKind::Image(i) => {
            let _ = write!(line, " {} {}×{}", i.asset, num(i.width), num(i.height));
        }
        NodeKind::Text(t) => {
            let preview = truncate(&t.content.replace('\n', "⏎"), opts.text_preview);
            let _ = write!(
                line,
                " \"{}\" {}px/{} {}",
                preview,
                num(t.font.font_size),
                t.font.font_weight,
                t.font.font_family
            );
        }
        NodeKind::Path(p) => {
            // Anchor count is the useful summary; the actual path data is rarely what a
            // model needs and is always the longest thing in the file.
            let anchors = p.d.matches(|c| c == 'C' || c == 'L' || c == 'M').count();
            let _ = write!(line, " {anchors} segments");
        }
        NodeKind::Group => {}
    }

    if let Some(fill) = node.fills.first() {
        let _ = write!(line, " fill:{}", paint_summary(fill));
        if node.fills.len() > 1 {
            let _ = write!(line, "+{}", node.fills.len() - 1);
        }
    }
    if let Some(stroke) = node.strokes.first() {
        let _ = write!(line, " stroke:{}@{}", paint_summary(&stroke.paint), num(stroke.width));
    }
    if !node.transform.is_identity() {
        let d = node.transform.decompose();
        let _ = write!(line, " at({},{})", num(d.x), num(d.y));
        if d.rotation.abs() > 1e-6 {
            let _ = write!(line, " rot({}°)", num(d.rotation.to_degrees()));
        }
    }
    if node.opacity < 1.0 {
        let _ = write!(line, " opacity:{}", num(node.opacity));
    }
    if !node.visible {
        let _ = write!(line, " hidden");
    }
    if node.locked {
        let _ = write!(line, " locked");
    }
    if !node.effects.is_empty() {
        let _ = write!(line, " +{} effect(s)", node.effects.len());
    }
    if let Some(a) = &node.a11y {
        if let Some(role) = &a.role {
            let _ = write!(line, " aria:{role}");
        }
    }

    let _ = writeln!(out, "{line}");

    for child in &node.children {
        write_node(out, child, depth + 1, opts);
    }
}

fn write_timeline(out: &mut String, tl: &Timeline) {
    let mut line = format!(
        "    {} \"{}\" {} {}s {} track(s)",
        tl.id,
        tl.name,
        tl.trigger.type_name(),
        num(tl.duration),
        tl.tracks.len()
    );
    if let Some(src) = &tl.source {
        let _ = write!(line, " ← {}@{} on {}", src.package, src.version, src.target);
    }
    if !tl.enabled {
        let _ = write!(line, " (disabled)");
    }
    let unknown = tl.unknown_properties();
    if !unknown.is_empty() {
        let _ = write!(line, " ⚠ unhandled: {}", unknown.join(","));
    }
    let _ = writeln!(out, "{line}");
}

fn paint_summary(p: &Paint) -> String {
    match p {
        Paint::Solid { color, opacity } => {
            if (*opacity - 1.0).abs() < f64::EPSILON {
                color.to_string()
            } else {
                format!("{color}/{}", num(*opacity))
            }
        }
        Paint::LinearGradient { stops, .. } => format!("linear({} stops)", stops.len()),
        Paint::RadialGradient { stops, .. } => format!("radial({} stops)", stops.len()),
        Paint::Image { asset, .. } => format!("image({asset})"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

fn num(v: f64) -> String {
    md_geom::fmt_coord(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{Keyframe, Timeline, Track, Trigger};
    use crate::id::{NodeId, TimelineId};
    use crate::node::{NodeKind, RectGeometry};
    use crate::text::TextGeometry;
    use serde_json::json;

    fn doc() -> Document {
        let mut d = Document::new("Portfolio");
        let title = Node::new(
            NodeId::from_static("nd_title"),
            NodeKind::Text(TextGeometry::new("Design in motion", "Inter", 48.0)),
        )
        .with_role("hero-title");

        let card = Node::new(
            NodeId::from_static("nd_card"),
            NodeKind::Rect(RectGeometry {
                width: 320.0,
                height: 200.0,
                corner_radius: [12.0; 4],
            }),
        )
        .with_role("card")
        .with_fill(Paint::solid("#ffffff").unwrap());

        d.pages[0].root.id = NodeId::from_static("nd_root");
        d.pages[0].root.children.push(title);
        d.pages[0].root.children.push(card);
        d
    }

    #[test]
    fn header_states_the_shape_of_the_document() {
        let out = digest(&doc(), &DigestOptions::default());
        assert!(out.contains("document \"Portfolio\""), "got:\n{out}");
        assert!(out.contains("1 page"), "got:\n{out}");
    }

    #[test]
    fn the_role_vocabulary_is_listed_up_front() {
        let out = digest(&doc(), &DigestOptions::default());
        assert!(out.contains("roles: @card @hero-title"), "got:\n{out}");
    }

    #[test]
    fn nodes_show_type_id_role_and_size() {
        let out = digest(&doc(), &DigestOptions::default());
        assert!(out.contains("rect nd_card @card 320×200 r:12 fill:#ffffff"), "got:\n{out}");
        assert!(out.contains("text nd_title @hero-title \"Design in motion\" 48px/400 Inter"), "got:\n{out}");
    }

    #[test]
    fn depth_limit_summarizes_rather_than_truncates_silently() {
        let mut d = doc();
        let deep = Node::new(NodeId::from_static("nd_deep"), NodeKind::Group)
            .with_children(vec![Node::new(NodeId::from_static("nd_deeper"), NodeKind::Group)]);
        d.pages[0].root.children.push(deep);

        let out = digest(&d, &DigestOptions { max_depth: 1, ..Default::default() });
        assert!(out.contains("more node(s) below this depth"), "got:\n{out}");
    }

    #[test]
    fn timelines_report_the_package_they_came_from() {
        let mut d = doc();
        d.pages[0].timelines.push(Timeline {
            id: TimelineId::from_static("tl_hero"),
            name: "Hero in".into(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 0.8,
            enabled: true,
            reduced_motion: Default::default(),
            source: Some(crate::anim::AnimSource {
                package: "std/stagger-fade-up".into(),
                version: "1.0.0".into(),
                target: "@card".into(),
                params: Default::default(),
            }),
            tracks: vec![Track {
                target: NodeId::from_static("nd_card"),
                property: "opacity".into(),
                keyframes: vec![Keyframe {
                    t: 0.0,
                    value: json!(0),
                    easing: Default::default(),
                }],
            }],
        });

        let out = digest(&d, &DigestOptions::default());
        assert!(out.contains("← std/stagger-fade-up@1.0.0 on @card"), "got:\n{out}");
    }

    #[test]
    fn a_digest_is_far_smaller_than_the_json_it_describes() {
        let d = doc();
        let json = crate::canonical::to_canonical_string(&d).unwrap();
        let text = digest(&d, &DigestOptions::default());
        assert!(
            text.len() * 2 < json.len(),
            "digest {} bytes vs json {} bytes — not earning its keep",
            text.len(),
            json.len()
        );
    }

    #[test]
    fn page_filter_selects_one_page() {
        let out = digest(&doc(), &DigestOptions { page: Some("nope".into()), ..Default::default() });
        assert!(!out.contains("page \""), "got:\n{out}");
    }
}
