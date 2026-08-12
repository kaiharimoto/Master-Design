//! The page shell: document head, stylesheet, and the accessibility outline.

use crate::svg::{esc_attr, esc_text, num};
use md_doc::node::{Node, NodeKind};
use md_doc::{Color, Document, Page};
use std::fmt::Write;

/// Assemble a complete HTML document.
pub fn page_html(
    doc: &Document,
    page: &Page,
    svg: &str,
    animations: Option<&str>,
    runtime: Option<&str>,
    outline: bool,
    // `@font-face` rules for the faces this page uses, or empty.
    font_css: &str,
) -> String {
    let title = if page.name.is_empty() {
        doc.meta.name.clone()
    } else {
        page.name.clone()
    };

    let description = if doc.meta.description.is_empty() {
        String::new()
    } else {
        format!(
            "<meta name=\"description\" content=\"{}\">",
            esc_attr(&doc.meta.description)
        )
    };

    let a11y = if outline {
        let body = accessibility_outline(page);
        if body.is_empty() {
            String::new()
        } else {
            format!("<div class=\"md-a11y\">{body}</div>")
        }
    } else {
        String::new()
    };

    let anim_payload = animations
        .map(|json| {
            format!(
                "<script type=\"application/json\" id=\"md-animations\">{}</script>",
                escape_in_script(json)
            )
        })
        .unwrap_or_default();

    let runtime_tag = runtime
        .map(|js| format!("<script>{js}</script>"))
        .unwrap_or_default();

    let css = format!("{font_css}{}", stylesheet(page));

    format!(
        "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{title}</title>\n\
{description}\n\
<style>{css}</style>\n\
</head>\n\
<body>\n\
{a11y}\
<main class=\"md-page\">{svg}</main>\n\
{anim_payload}\n\
{runtime_tag}\n\
</body>\n\
</html>\n",
        title = esc_text(&title),
        // Font rules first: a browser starts fetching a font the moment it sees the rule,
        // and everything after it in the sheet is cheap by comparison.
        css = css,
    )
}

/// Make a JSON payload safe to sit inside a `<script>` element.
///
/// The HTML parser looks for `</script` inside a script element before any JSON parser
/// gets a say, and `serde_json` has no reason to escape `<`. A keyframe value is an
/// arbitrary string from the document, so without this a fill colour spelled
/// `</script><script>…` would close the element and run as markup. The unicode escapes
/// are the same string to a JSON parser and inert to the HTML one.
fn escape_in_script(json: &str) -> String {
    json.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

/// A solid paint as a CSS colour, keeping whatever alpha it carries.
///
/// The alpha can live in two places — the paint's `opacity` and the last two hex digits
/// of the colour — and CSS `background` takes one value, so the two are multiplied.
/// Taking only `#rrggbb` exported every translucent background fully opaque.
fn css_color(color: &Color, opacity: f64) -> String {
    let [r, g, b, a] = color.rgba();
    let alpha = a * opacity;
    if alpha >= 1.0 {
        return color.hex_rgb();
    }
    format!(
        "rgba({},{},{},{})",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        num(alpha)
    )
}

/// Base stylesheet. Short on purpose — the design is in the SVG, and CSS here exists
/// only to place it and to give the animation runtime somewhere safe to work.
pub fn stylesheet(page: &Page) -> String {
    let background = page
        .background
        .as_ref()
        .and_then(|p| match p {
            md_doc::Paint::Solid { color, opacity } => Some(css_color(color, *opacity)),
            _ => None,
        })
        .unwrap_or_else(|| "#ffffff".to_string());

    // Explanations live here rather than in the emitted CSS: every byte of comment would
    // ship on every page view, and the reasoning is only useful to whoever maintains
    // this function.
    //
    // - `[data-md-fx]` is the animation wrapper, and the only place a compositing hint
    //   is warranted. Promoting every layer would cost memory for nothing.
    // - `.md-a11y` is clipped rather than `display:none`, which would take it out of the
    //   accessibility tree — the exact opposite of what it is for. It un-hides on focus
    //   so a keyboard user can see where they are if it ever contains a control.
    format!(
        "*,*::before,*::after{{box-sizing:border-box}}\
html,body{{margin:0;padding:0}}\
body{{background:{background};min-height:100%}}\
.md-page{{display:block;width:100%}}\
.md-canvas{{display:block;width:100%;height:auto}}\
[data-md-fx]{{transform-box:fill-box;transform-origin:50% 50%;will-change:transform,opacity}}\
.md-a11y{{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;\
clip-path:inset(50%);white-space:nowrap;border:0}}\
.md-a11y:focus-within{{position:static;width:auto;height:auto;margin:0;clip-path:none;\
white-space:normal}}\
@media (prefers-reduced-motion:reduce){{[data-md-fx]{{will-change:auto}}}}"
    )
}

/// A parallel semantic document, visually hidden.
///
/// SVG is a poor carrier of meaning: a heading drawn as vector text is, to a screen
/// reader or a search engine, a shape. Rather than pretend otherwise, the export ships
/// the structure separately — real `<h1>`s, paragraphs, labels and alternative text, in
/// document order — while the SVG stays marked `aria-hidden`.
///
/// The alternative, littering the SVG with ARIA, works far less well in practice: the
/// support is patchy and the resulting reading order follows paint order rather than
/// meaning.
pub fn accessibility_outline(page: &Page) -> String {
    let mut out = String::new();
    walk(&page.root, &mut out);
    out
}

fn walk(node: &Node, out: &mut String) {
    if !node.visible {
        return;
    }
    if let Some(a) = &node.a11y {
        if a.hidden {
            return;
        }
    }

    let landmark = node
        .a11y
        .as_ref()
        .and_then(|a| a.role.as_deref())
        .and_then(landmark_tag);

    if let Some(tag) = landmark {
        let label = node
            .a11y
            .as_ref()
            .and_then(|a| a.label.as_deref())
            .map(|l| format!(" aria-label=\"{}\"", esc_attr(l)))
            .unwrap_or_default();
        let _ = write!(out, "<{tag}{label}>");
    }

    // A container's own label belongs to the landmark that wraps its children, not to a
    // separate graphic. Emitting both would announce "Primary" twice.
    if landmark.is_none() || !node.is_container() {
        emit_content(node, out);
    }

    for child in &node.children {
        walk(child, out);
    }

    if let Some(tag) = landmark {
        let _ = write!(out, "</{tag}>");
    }
}

fn emit_content(node: &Node, out: &mut String) {
    let a11y = node.a11y.as_ref();

    match &node.kind {
        NodeKind::Text(t) => {
            if t.content.trim().is_empty() {
                return;
            }
            let text = esc_text(&t.content);
            match a11y.and_then(|a| a.heading_level) {
                Some(level) => {
                    let level = level.clamp(1, 6);
                    let _ = write!(out, "<h{level}>{text}</h{level}>");
                }
                None => {
                    let _ = write!(out, "<p>{text}</p>");
                }
            }
        }
        NodeKind::Image(i) => {
            let alt = a11y.and_then(|a| a.alt.as_deref()).unwrap_or("");
            if alt.is_empty() {
                // No alt and not marked decorative: say something rather than nothing,
                // so the gap is audible instead of invisible.
                let _ = write!(
                    out,
                    "<img src=\"assets/{}\" alt=\"\" role=\"presentation\">",
                    esc_attr(&i.asset)
                );
            } else {
                let _ = write!(
                    out,
                    "<img src=\"assets/{}\" alt=\"{}\">",
                    esc_attr(&i.asset),
                    esc_attr(alt)
                );
            }
        }
        NodeKind::Group | NodeKind::Frame(_) => {}
        _ => {
            // A shape with a label is meaningful artwork — a logo, an icon, a diagram.
            if let Some(label) = a11y.and_then(|a| a.label.as_deref().or(a.alt.as_deref())) {
                let _ = write!(
                    out,
                    "<p role=\"img\" aria-label=\"{}\"></p>",
                    esc_attr(label)
                );
            }
        }
    }
}

/// Map an ARIA role onto the HTML element that carries it natively.
fn landmark_tag(role: &str) -> Option<&'static str> {
    match role {
        "banner" | "header" => Some("header"),
        "navigation" | "nav" => Some("nav"),
        "main" => Some("main"),
        "contentinfo" | "footer" => Some("footer"),
        "complementary" | "aside" => Some("aside"),
        "region" | "section" => Some("section"),
        "article" => Some("article"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::node::{NodeKind, RectGeometry};
    use md_doc::text::TextGeometry;
    use md_doc::{A11y, Color, Document, Node, NodeId, PageId, Paint};

    fn page() -> Page {
        let mut p = Page::new(PageId::from_static("pg_1"), "Home", "index", 1440.0, 900.0);

        let mut heading = Node::new(
            NodeId::from_static("nd_h"),
            NodeKind::Text(TextGeometry::new("Design in motion", "Inter", 64.0)),
        );
        heading.a11y = Some(A11y {
            heading_level: Some(1),
            ..Default::default()
        });

        let body = Node::new(
            NodeId::from_static("nd_p"),
            NodeKind::Text(TextGeometry::new("Everything moves.", "Inter", 18.0)),
        );

        let mut nav = Node::new(NodeId::from_static("nd_nav"), NodeKind::Group);
        nav.a11y = Some(A11y {
            role: Some("navigation".into()),
            label: Some("Primary".into()),
            ..Default::default()
        });

        p.root.children.push(nav);
        p.root.children.push(heading);
        p.root.children.push(body);
        p
    }

    #[test]
    fn headings_become_real_heading_elements() {
        let out = accessibility_outline(&page());
        assert!(out.contains("<h1>Design in motion</h1>"), "got {out}");
    }

    #[test]
    fn plain_text_becomes_a_paragraph() {
        let out = accessibility_outline(&page());
        assert!(out.contains("<p>Everything moves.</p>"), "got {out}");
    }

    #[test]
    fn landmark_roles_become_landmark_elements() {
        let out = accessibility_outline(&page());
        assert!(out.contains("<nav aria-label=\"Primary\">"), "got {out}");
    }

    #[test]
    fn nodes_marked_decorative_are_left_out() {
        let mut p = page();
        let mut decorative = Node::new(
            NodeId::from_static("nd_d"),
            NodeKind::Text(TextGeometry::new("swoosh", "Inter", 12.0)),
        );
        decorative.a11y = Some(A11y {
            hidden: true,
            ..Default::default()
        });
        p.root.children.push(decorative);

        assert!(!accessibility_outline(&p).contains("swoosh"));
    }

    #[test]
    fn hidden_layers_are_left_out() {
        let mut p = page();
        let mut invisible = Node::new(
            NodeId::from_static("nd_i"),
            NodeKind::Text(TextGeometry::new("draft copy", "Inter", 12.0)),
        );
        invisible.visible = false;
        p.root.children.push(invisible);

        assert!(!accessibility_outline(&p).contains("draft copy"));
    }

    #[test]
    fn shapes_with_no_meaning_contribute_nothing() {
        let mut p = page();
        p.root.children.push(Node::new(
            NodeId::from_static("nd_r"),
            NodeKind::Rect(RectGeometry {
                width: 10.0,
                height: 10.0,
                corner_radius: [0.0; 4],
            }),
        ));
        let out = accessibility_outline(&p);
        assert!(!out.contains("role=\"img\""), "got {out}");
    }

    #[test]
    fn text_is_escaped() {
        let mut p = Page::new(PageId::from_static("pg_1"), "Home", "index", 100.0, 100.0);
        p.root.children.push(Node::new(
            NodeId::from_static("nd_t"),
            NodeKind::Text(TextGeometry::new(
                "<script>alert(1)</script>",
                "Inter",
                12.0,
            )),
        ));
        let out = accessibility_outline(&p);
        assert!(
            !out.contains("<script>"),
            "unescaped markup got through: {out}"
        );
        assert!(out.contains("&lt;script&gt;"), "got {out}");
    }

    #[test]
    fn the_hidden_outline_stays_in_the_accessibility_tree() {
        let css = stylesheet(&page());
        assert!(css.contains("clip-path:inset(50%)"), "got {css}");
        assert!(
            !css.contains("display:none"),
            "display:none would hide it from screen readers"
        );
    }

    #[test]
    fn a_keyframe_cannot_break_out_of_the_animation_payload() {
        // A `fill` keyframe is an arbitrary string that reaches the payload verbatim,
        // and serde_json leaves `<` and `>` alone.
        let payload = serde_json::to_string(&serde_json::json!({
            "entries": [{
                "selector": "[data-md-fx=\"nd_a\"]",
                "keyframes": [{ "fill": "</script><script>alert(1)</script>" }],
            }],
        }))
        .unwrap();

        let html = page_html(
            &Document::new("Test"),
            &page(),
            "<svg></svg>",
            Some(&payload),
            None,
            false,
            "",
        );

        assert!(
            !html.contains("<script>alert(1)"),
            "the payload closed its own element and injected a script: {html}"
        );
        assert!(
            html.contains("\\u003c/script\\u003e"),
            "the escape did not survive into the page: {html}"
        );
    }

    #[test]
    fn a_translucent_page_background_keeps_its_alpha() {
        let mut p = page();
        p.background = Some(Paint::Solid {
            color: Color::parse("#0b102080").unwrap(),
            opacity: 1.0,
        });
        let css = stylesheet(&p);
        assert!(
            css.contains("background:rgba(11,16,32,0.502)"),
            "a translucent background exported opaque: {css}"
        );
    }

    #[test]
    fn an_opaque_page_background_stays_a_plain_hex() {
        let mut p = page();
        p.background = Some(Paint::solid("#0b1020").unwrap());
        let css = stylesheet(&p);
        assert!(css.contains("background:#0b1020"), "got {css}");
    }
}
