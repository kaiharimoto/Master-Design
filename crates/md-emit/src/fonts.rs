//! Shipping the typeface with the page.
//!
//! An exported page that says `font-family: Inter` and stops there is a page that renders
//! in Inter on the designer's machine and in Arial on everyone else's — with different
//! metrics, so every line wraps differently and every centred heading sits somewhere
//! else. The design was made against real measurements; the page has to be able to
//! reproduce them.
//!
//! So the exporter copies the font files the document actually uses into `dist/fonts/`
//! and writes matching `@font-face` rules. The files are the same `.woff2` `md-text`
//! measured with, which is what makes the two agree.
//!
//! Only fonts the tool ships or the project carries are embedded. A font the designer
//! installed on their own machine is licensed to *them*, and copying it into a website
//! they publish would be this program making a licensing decision on their behalf. When
//! that happens the export says so and falls back to a font stack.

use md_doc::node::NodeKind;
use md_doc::{Document, Page};
use std::collections::BTreeSet;

/// Directory, relative to the page, that fonts are written into.
pub const FONTS_DIR: &str = "fonts";

/// One face a page needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FaceRequest {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

/// Every distinct face the text on these pages asks for.
///
/// Distinct, because a page with forty paragraphs in one style must not copy the same
/// file forty times, and because `@font-face` rules are matched by the browser rather
/// than applied in order.
pub fn faces_used(pages: &[&Page]) -> BTreeSet<FaceRequest> {
    let mut out = BTreeSet::new();
    for page in pages {
        collect(&page.root, &mut out);
    }
    out
}

fn collect(node: &md_doc::Node, out: &mut BTreeSet<FaceRequest>) {
    if let NodeKind::Text(t) = &node.kind {
        // Hidden text still counts: a layer toggled off in the editor is one keystroke
        // from being on, and a page that changes its font when a layer is revealed is a
        // worse surprise than one extra 24 kB file.
        out.insert(FaceRequest {
            family: t.font.font_family.clone(),
            weight: t.font.font_weight,
            italic: t.font.italic,
        });
        for span in &t.spans {
            out.insert(FaceRequest {
                family: span
                    .font_family
                    .clone()
                    .unwrap_or_else(|| t.font.font_family.clone()),
                weight: span.font_weight.unwrap_or(t.font.font_weight),
                italic: span.italic.unwrap_or(t.font.italic),
            });
        }
    }
    for child in &node.children {
        collect(child, out);
    }
}

/// A font file to write, and the `@font-face` rule that points at it.
#[derive(Debug, Clone)]
pub struct Embedded {
    pub file_name: String,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub css: String,
}

/// Work out what to ship for a document.
///
/// Returns the files to write, the stylesheet fragment, and anything the user should know
/// about a face that could not be carried.
pub fn plan(doc: &Document, only_page: Option<&str>) -> (Vec<Embedded>, Vec<String>) {
    let pages: Vec<&Page> = doc
        .pages
        .iter()
        .filter(|p| match only_page {
            Some(key) => p.id.as_str() == key || p.slug == key,
            None => true,
        })
        .collect();

    let fonts = md_text::shared();
    let mut files: Vec<Embedded> = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_files = BTreeSet::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();

    for face in faces_used(&pages) {
        let Some(web) = fonts.web_font(&face.family, face.weight, face.italic) else {
            // One line per family, not per weight: a document using six weights of a
            // system font would otherwise bury everything else in the report.
            if reported.insert(face.family.to_lowercase()) {
                warnings.push(format!(
                    "\"{}\" could not be embedded — it is not one of the fonts this tool \
                     ships or one carried in the project's fonts/ folder, so the page will \
                     use whatever the visitor happens to have. Put a .woff2 in the \
                     project's fonts/ folder to ship it.",
                    face.family
                ));
            }
            continue;
        };

        // A weight the family does not have resolves to its nearest neighbour, so several
        // requests can land on one file. Write it once, and let the `@font-face` rule
        // claim the requested weight so the browser makes the same substitution.
        let css = font_face_rule(&face, &web.file_name);
        if seen_files.insert(web.file_name.clone()) {
            files.push(Embedded {
                file_name: web.file_name.clone(),
                bytes: web.bytes.clone(),
                css,
            });
        } else if let Some(existing) = files.iter_mut().find(|f| f.file_name == web.file_name) {
            let rule = font_face_rule(&face, &web.file_name);
            if !existing.css.contains(&rule) {
                existing.css.push_str(&rule);
            }
        }
    }

    (files, warnings)
}

fn font_face_rule(face: &FaceRequest, file_name: &str) -> String {
    format!(
        "@font-face{{font-family:\"{}\";font-style:{};font-weight:{};font-display:swap;\
         src:url(\"{FONTS_DIR}/{}\") format(\"woff2\")}}",
        face.family.replace('"', ""),
        if face.italic { "italic" } else { "normal" },
        face.weight,
        file_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::text::TextGeometry;
    use md_doc::{Node, NodeId, PageId};

    fn doc_with(texts: Vec<TextGeometry>) -> Document {
        let mut d = Document::new("Fonts");
        d.pages[0] = Page::new(PageId::from_static("pg_1"), "Home", "index", 800.0, 600.0);
        for (i, t) in texts.into_iter().enumerate() {
            d.pages[0].root.children.push(Node::new(
                NodeId::parse(format!("nd_t{i}")).unwrap(),
                NodeKind::Text(t),
            ));
        }
        d
    }

    fn text(family: &str, weight: u16, italic: bool) -> TextGeometry {
        let mut t = TextGeometry::new("Hello", family, 16.0);
        t.font.font_weight = weight;
        t.font.italic = italic;
        t
    }

    #[test]
    fn a_page_ships_the_face_its_text_asks_for() {
        let (files, warnings) = plan(&doc_with(vec![text("Inter", 400, false)]), None);
        assert_eq!(files.len(), 1);
        assert!(files[0].file_name.ends_with(".woff2"));
        assert!(files[0].css.contains("font-weight:400"));
        assert!(files[0].css.contains("format(\"woff2\")"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn one_file_is_written_however_many_nodes_use_it() {
        let d = doc_with(vec![
            text("Inter", 400, false),
            text("Inter", 400, false),
            text("Inter", 400, false),
        ]);
        assert_eq!(plan(&d, None).0.len(), 1);
    }

    #[test]
    fn distinct_weights_ship_distinct_files() {
        let d = doc_with(vec![text("Inter", 400, false), text("Inter", 700, false)]);
        let (files, _) = plan(&d, None);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn a_weight_that_falls_back_still_claims_the_weight_it_was_asked_for() {
        // 900 is not bundled and resolves to 800. The rule has to say 900, or the browser
        // will synthesise a bolder face on top of the already-bold file.
        let d = doc_with(vec![text("Inter", 900, false)]);
        let (files, _) = plan(&d, None);
        assert_eq!(files.len(), 1);
        assert!(
            files[0].css.contains("font-weight:900"),
            "got {}",
            files[0].css
        );
    }

    #[test]
    fn two_requests_landing_on_one_file_produce_two_rules() {
        // 800 and 900 both resolve to the 800 file, and the browser needs a rule for each
        // or `font-weight: 900` matches nothing.
        let d = doc_with(vec![text("Inter", 800, false), text("Inter", 900, false)]);
        let (files, _) = plan(&d, None);
        assert_eq!(files.len(), 1, "the same file was written twice");
        assert!(files[0].css.contains("font-weight:800"));
        assert!(files[0].css.contains("font-weight:900"));
    }

    #[test]
    fn a_font_that_cannot_be_shipped_is_reported_rather_than_dropped() {
        let d = doc_with(vec![text("Helvetica Neue", 400, false)]);
        let (files, warnings) = plan(&d, None);
        assert!(files.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Helvetica Neue"), "{warnings:?}");
        assert!(
            warnings[0].contains("fonts/"),
            "the warning should say what to do about it: {warnings:?}"
        );
    }

    #[test]
    fn an_unshippable_family_is_reported_once_not_once_per_weight() {
        let d = doc_with(vec![
            text("Helvetica Neue", 400, false),
            text("Helvetica Neue", 700, false),
            text("Helvetica Neue", 300, true),
        ]);
        let (_, warnings) = plan(&d, None);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn italics_get_their_own_rule() {
        let d = doc_with(vec![text("Inter", 400, false), text("Inter", 400, true)]);
        let (files, _) = plan(&d, None);
        let all: String = files.iter().map(|f| f.css.clone()).collect();
        assert!(all.contains("font-style:italic"));
        assert!(all.contains("font-style:normal"));
    }

    #[test]
    fn exporting_one_page_ships_only_that_page_s_fonts() {
        let mut d = doc_with(vec![text("Inter", 400, false)]);
        let mut second = Page::new(PageId::from_static("pg_2"), "About", "about", 800.0, 600.0);
        second.root.children.push(Node::new(
            NodeId::from_static("nd_about"),
            NodeKind::Text(text("Inter", 800, false)),
        ));
        d.pages.push(second);

        assert_eq!(plan(&d, Some("index")).0.len(), 1);
        assert_eq!(plan(&d, None).0.len(), 2);
    }

    #[test]
    fn a_hidden_layer_s_font_is_still_shipped() {
        let mut d = doc_with(vec![text("Inter", 400, false)]);
        let mut hidden = Node::new(
            NodeId::from_static("nd_hidden"),
            NodeKind::Text(text("Inter", 800, false)),
        );
        hidden.visible = false;
        d.pages[0].root.children.push(hidden);

        assert_eq!(
            plan(&d, None).0.len(),
            2,
            "revealing a layer must not change the page's typography"
        );
    }

    #[test]
    fn a_span_that_changes_weight_brings_its_own_face() {
        let mut t = text("Inter", 400, false);
        t.spans.push(md_doc::text::TextSpan {
            start: 0,
            end: 2,
            font_family: None,
            font_size: None,
            font_weight: Some(700),
            italic: None,
            fill: None,
            href: None,
        });
        assert_eq!(plan(&doc_with(vec![t]), None).0.len(), 2);
    }

    #[test]
    fn a_quote_in_a_family_name_cannot_break_out_of_the_rule() {
        let d = doc_with(vec![text("Inter\";}body{display:none", 400, false)]);
        let (files, _) = plan(&d, None);
        for file in &files {
            assert!(
                !file.css.contains("body{display:none"),
                "a family name escaped its quotes: {}",
                file.css
            );
        }
    }
}
