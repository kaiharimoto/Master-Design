//! # md-emit
//!
//! Turns a document into a self-contained static website: HTML, CSS, inline SVG, and a
//! small animation runtime, with no build step and no framework.
//!
//! Everything here runs in Rust with no JavaScript engine involved, which is what lets
//! `md-cli export` and the MCP server produce a page — and a screenshot of one — on a
//! machine with nothing but the binary. See [`anim`] for how timelines are resolved
//! ahead of time so the shipped runtime stays trivial.

pub mod anim;
pub mod fonts;
pub mod frame;
pub mod html;
pub mod svg;

use md_doc::{Document, Page};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The animation runtime, inlined into every page that needs it.
///
/// Kept next to the exporter rather than in a JavaScript package because it has exactly
/// one consumer and no build step: `include_str!` beats a cross-language build ordering
/// problem for four kilobytes of code.
pub const RUNTIME_JS: &str = include_str!("../assets/runtime.js");

#[derive(Debug, Error)]
pub enum EmitError {
    #[error("{0}")]
    Io(String),

    #[error(transparent)]
    Doc(#[from] md_doc::DocError),
}

impl From<std::io::Error> for EmitError {
    fn from(e: std::io::Error) -> Self {
        EmitError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, EmitError>;

#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Emit the hidden semantic outline alongside the artwork.
    pub accessibility_outline: bool,
    /// Copy the project's `assets/` directory into the output.
    pub assets_from: Option<PathBuf>,
    /// Export only this page, by id or slug.
    pub only_page: Option<String>,
    /// Copy the fonts the document uses into the output and reference them.
    ///
    /// On by default: a page that merely names a font renders in whatever the visitor
    /// happens to have, with different metrics, so every line wraps somewhere else than
    /// the designer saw. Turning it off is for a site that already serves its own fonts
    /// and does not want a second copy.
    pub embed_fonts: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            accessibility_outline: true,
            assets_from: None,
            only_page: None,
            embed_fonts: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    pub files: Vec<PathBuf>,
    pub bytes: usize,
    /// Things worth knowing that did not stop the export — an animation property the
    /// exporter cannot compile, a stroke alignment it had to approximate.
    pub warnings: Vec<String>,
}

/// Render just the `<svg>` for a page.
///
/// Separate from [`render_page`] because the snapshot renderer wants the artwork without
/// a document around it — and wants the page colour painted into it, which an exported
/// page leaves to CSS. Pulling the SVG back out of the HTML with a string search, which
/// is what this replaces, was one malformed `</svg>` away from silent nonsense.
pub fn render_svg(doc: &Document, page: &Page, paint_background: bool) -> (String, Vec<String>) {
    let compiled = anim::compile(doc, page);
    let mut writer = svg::SvgWriter::new(&compiled.dashed, &compiled.animated);
    let markup = writer.page(page, paint_background);

    let mut warnings = compiled.warnings.clone();
    warnings.extend(writer.warnings.clone());
    (markup, warnings)
}

/// Render one page to a complete HTML document.
///
/// Separate from [`export`] because the MCP snapshot tool and the studio's preview both
/// want the markup without anything touching the filesystem.
pub fn render_page(doc: &Document, page: &Page, opts: &ExportOptions) -> (String, Vec<String>) {
    let compiled = anim::compile(doc, page);

    let mut writer = svg::SvgWriter::new(&compiled.dashed, &compiled.animated);
    // The stylesheet paints the page colour; see `SvgWriter::page`.
    let markup = writer.page(page, false);

    let mut warnings = compiled.warnings.clone();
    warnings.extend(writer.warnings.clone());

    // The `@font-face` rules go in the page even when nothing writes the files — this
    // function is also how the studio previews a page, and a preview with no typography
    // is not a preview. `export` writes the files that back them.
    let font_css = if opts.embed_fonts {
        let (files, font_warnings) = fonts::plan(doc, Some(page.slug.as_str()));
        warnings.extend(font_warnings);
        files.iter().map(|f| f.css.clone()).collect::<String>()
    } else {
        String::new()
    };

    let payload = compiled.to_payload();
    let (animations, runtime) = if compiled.is_empty() {
        (None, None)
    } else {
        (Some(payload.as_str()), Some(RUNTIME_JS))
    };

    let html = html::page_html(
        doc,
        page,
        &markup,
        animations,
        runtime,
        opts.accessibility_outline,
        &font_css,
    );

    (html, warnings)
}

/// Write the whole site to a directory.
pub fn export(doc: &Document, out_dir: &Path, opts: &ExportOptions) -> Result<ExportReport> {
    doc.validate()?;
    fs::create_dir_all(out_dir)?;

    let mut report = ExportReport::default();

    for page in &doc.pages {
        if let Some(only) = &opts.only_page {
            if page.id.as_str() != only && page.slug != *only {
                continue;
            }
        }

        let (html, warnings) = render_page(doc, page, opts);
        report
            .warnings
            .extend(warnings.into_iter().map(|w| format!("{}: {w}", page.name)));

        let path = out_dir.join(page.output_file());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &html)?;
        report.bytes += html.len();
        report.files.push(path);
    }

    if opts.embed_fonts {
        // Written once for the whole site rather than per page: several pages usually
        // share a typeface, and a browser caches one URL.
        let (files, _) = fonts::plan(doc, opts.only_page.as_deref());
        if !files.is_empty() {
            let dir = out_dir.join(fonts::FONTS_DIR);
            fs::create_dir_all(&dir)?;
            for font in files {
                let path = dir.join(&font.file_name);
                fs::write(&path, font.bytes.as_slice())?;
                report.bytes += font.bytes.len();
                report.files.push(path);
            }
        }
    }

    if let Some(assets) = &opts.assets_from {
        let target = out_dir.join("assets");
        let copied = copy_dir(assets, &target)?;
        report.files.extend(copied);
    }

    if report.files.is_empty() {
        report
            .warnings
            .push("nothing was exported — check that the requested page exists".to_string());
    }

    Ok(report)
}

fn copy_dir(from: &Path, to: &Path) -> Result<Vec<PathBuf>> {
    let mut copied = Vec::new();
    let entries = match fs::read_dir(from) {
        Ok(e) => e,
        // A project with no assets directory is normal.
        Err(_) => return Ok(copied),
    };

    fs::create_dir_all(to)?;
    for entry in entries.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copied.extend(copy_dir(&src, &dst)?);
        } else {
            fs::copy(&src, &dst)?;
            copied.push(dst);
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::anim::{Keyframe, Timeline, Track, Trigger};
    use md_doc::node::{NodeKind, PathGeometry, RectGeometry};
    use md_doc::text::TextGeometry;
    use md_doc::{A11y, Node, NodeId, Paint, Stroke, TimelineId, Transform};
    use serde_json::json;

    fn doc() -> Document {
        let mut d = Document::new("Portfolio");
        d.meta.description = "A test site".into();
        d.pages[0].root.id = NodeId::from_static("nd_root");
        d.pages[0].background = Some(Paint::solid("#0b1020").unwrap());

        let mut heading = Node::new(
            NodeId::from_static("nd_title"),
            NodeKind::Text(TextGeometry::new("Design in motion", "Inter", 64.0)),
        )
        .with_role("hero-title")
        .with_fill(Paint::solid("#ffffff").unwrap());
        heading.a11y = Some(A11y {
            heading_level: Some(1),
            ..Default::default()
        });
        heading.transform = Transform::translate(120.0, 200.0);

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

        let mut squiggle = Node::new(
            NodeId::from_static("nd_line"),
            NodeKind::Path(PathGeometry {
                d: "M 0 0 C 50 100 150 -100 200 0".into(),
                fill_rule: Default::default(),
            }),
        );
        squiggle
            .strokes
            .push(Stroke::new(Paint::solid("#ff0055").unwrap(), 4.0));

        d.pages[0].root.children.push(heading);
        d.pages[0].root.children.push(card);
        d.pages[0].root.children.push(squiggle);
        d
    }

    fn with_timeline(mut d: Document) -> Document {
        d.pages[0].timelines.push(Timeline {
            id: TimelineId::from_static("tl_in"),
            name: "Cards in".into(),
            trigger: Trigger::View {
                threshold: 0.2,
                once: true,
            },
            duration: 0.6,
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
                            easing: md_doc::Easing::EaseOut,
                        },
                        Keyframe {
                            t: 1.0,
                            value: json!(1),
                            easing: md_doc::Easing::Linear,
                        },
                    ],
                },
                Track {
                    target: NodeId::from_static("nd_card"),
                    property: "translateY".into(),
                    keyframes: vec![
                        Keyframe {
                            t: 0.0,
                            value: json!(32),
                            easing: md_doc::Easing::EaseOut,
                        },
                        Keyframe {
                            t: 1.0,
                            value: json!(0),
                            easing: md_doc::Easing::Linear,
                        },
                    ],
                },
            ],
        });
        d
    }

    fn render(d: &Document) -> String {
        render_page(d, &d.pages[0], &ExportOptions::default()).0
    }

    #[test]
    fn a_page_renders_to_a_complete_html_document() {
        let html = render(&doc());
        assert!(html.starts_with("<!doctype html>"), "got {}", &html[..40]);
        assert!(html.contains("<title>Home</title>"));
        assert!(html.contains("viewBox=\"0 0 1440 900\""));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn nodes_carry_their_ids_so_the_runtime_can_find_them() {
        let html = render(&doc());
        assert!(html.contains("data-md-id=\"nd_card\""), "got {html}");
    }

    #[test]
    fn live_shapes_are_baked_to_paths() {
        let html = render(&doc());
        // The card is a 320×200 rounded rect; it should appear as real path geometry,
        // starting where the left edge meets the top-left corner arc.
        assert!(
            html.contains("<path d=\"M 0 12 C"),
            "rounded rect was not baked: {html}"
        );
    }

    #[test]
    fn text_stays_selectable_text() {
        let html = render(&doc());
        assert!(html.contains("<text"), "text was rasterized or dropped");
        assert!(html.contains("Design in motion"));
    }

    /// Exactly one of the two owns the page colour, and it is CSS — the body extends
    /// past the artwork, so painting both would composite a translucent background
    /// against itself and leave a seam at the bottom edge of the canvas.
    #[test]
    fn the_page_background_is_painted_by_css_and_only_by_css() {
        let html = render(&doc());
        assert!(
            html.contains("background:#0b1020"),
            "body background missing: {html}"
        );
        assert!(
            !html.contains("<rect width=\"1440\" height=\"900\" fill=\"#0b1020\""),
            "the SVG painted the background too: {html}"
        );
    }

    /// …and the rasterizer, which has no stylesheet, still gets one.
    #[test]
    fn a_rendered_svg_carries_the_background_itself() {
        let d = doc();
        let (svg, _) = render_svg(&d, &d.pages[0], true);
        assert!(
            svg.contains("fill=\"#0b1020\""),
            "a standalone SVG lost the page colour: {svg}"
        );
    }

    #[test]
    fn a_page_with_no_animation_ships_no_runtime() {
        let html = render(&doc());
        assert!(
            !html.contains("md-animations"),
            "unused runtime payload was emitted"
        );
        assert!(
            !html.contains("master-design"),
            "unused runtime was emitted"
        );
    }

    #[test]
    fn an_animated_page_ships_the_runtime_and_its_payload() {
        let html = render(&with_timeline(doc()));
        assert!(html.contains("id=\"md-animations\""), "got {html}");
        assert!(
            html.contains("[data-md-fx=\\\"nd_card\\\"]"),
            "selector missing: {html}"
        );
    }

    #[test]
    fn animated_nodes_get_their_own_transform_wrapper() {
        // Without this the animation's `transform` would replace the node's placement
        // matrix and move it to the origin.
        let html = render(&with_timeline(doc()));
        assert!(html.contains("data-md-fx=\"nd_card\""), "got {html}");
        assert!(
            !html.contains("data-md-fx=\"nd_title\""),
            "only animated nodes need a wrapper"
        );
    }

    #[test]
    fn transform_components_are_merged_into_one_property() {
        let d = with_timeline(doc());
        let compiled = anim::compile(&d, &d.pages[0]);

        let transform_entries: Vec<_> = compiled
            .entries
            .iter()
            .filter(|e| e.keyframes.iter().any(|k| k.contains_key("transform")))
            .collect();
        assert_eq!(transform_entries.len(), 1);

        let first = &transform_entries[0].keyframes[0];
        assert_eq!(first["transform"], json!("translate(0px, 32px)"));
    }

    #[test]
    fn opacity_becomes_its_own_animation_so_its_easing_is_exact() {
        let d = with_timeline(doc());
        let compiled = anim::compile(&d, &d.pages[0]);
        let opacity = compiled
            .entries
            .iter()
            .find(|e| e.keyframes.iter().any(|k| k.contains_key("opacity")))
            .expect("no opacity animation");
        assert_eq!(opacity.keyframes[0]["easing"], json!("ease-out"));
    }

    #[test]
    fn a_draw_on_animation_gets_a_dash_array_to_offset() {
        let mut d = doc();
        d.pages[0].timelines.push(Timeline {
            id: TimelineId::from_static("tl_draw"),
            name: "Draw".into(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: true,
            reduced_motion: Default::default(),
            source: None,
            tracks: vec![Track {
                target: NodeId::from_static("nd_line"),
                property: "strokeDashoffset".into(),
                keyframes: vec![
                    Keyframe {
                        t: 0.0,
                        value: json!(250),
                        easing: md_doc::Easing::Linear,
                    },
                    Keyframe {
                        t: 1.0,
                        value: json!(0),
                        easing: md_doc::Easing::Linear,
                    },
                ],
            }],
        });

        let html = render_page(&d, &d.pages[0], &ExportOptions::default()).0;
        assert!(
            html.contains("stroke-dasharray="),
            "a stroke with nothing to offset would just look solid: {html}"
        );
    }

    #[test]
    fn disabled_timelines_are_skipped() {
        let mut d = with_timeline(doc());
        d.pages[0].timelines[0].enabled = false;
        let html = render_page(&d, &d.pages[0], &ExportOptions::default()).0;
        assert!(!html.contains("md-animations"));
    }

    #[test]
    fn unhandled_properties_are_reported_rather_than_dropped_silently() {
        let mut d = with_timeline(doc());
        d.pages[0].timelines[0].tracks.push(Track {
            target: NodeId::from_static("nd_card"),
            property: "wobbliness".into(),
            keyframes: vec![
                Keyframe {
                    t: 0.0,
                    value: json!(0),
                    easing: md_doc::Easing::Linear,
                },
                Keyframe {
                    t: 1.0,
                    value: json!(1),
                    easing: md_doc::Easing::Linear,
                },
            ],
        });

        let (_, warnings) = render_page(&d, &d.pages[0], &ExportOptions::default());
        assert!(
            warnings.iter().any(|w| w.contains("wobbliness")),
            "got {warnings:?}"
        );
    }

    #[test]
    fn the_accessibility_outline_is_included_by_default() {
        let html = render(&doc());
        assert!(html.contains("class=\"md-a11y\""), "got {html}");
        assert!(html.contains("<h1>Design in motion</h1>"));
        assert!(
            html.contains("aria-hidden=\"true\""),
            "the artwork should not be read twice"
        );
    }

    #[test]
    fn the_outline_can_be_turned_off() {
        let d = doc();
        let opts = ExportOptions {
            accessibility_outline: false,
            ..Default::default()
        };
        let (html, _) = render_page(&d, &d.pages[0], &opts);
        // The stylesheet still defines the class; what must be gone is the element.
        assert!(!html.contains("class=\"md-a11y\""), "got {html}");
        assert!(!html.contains("<h1>"));
    }

    #[test]
    fn exporting_writes_one_file_per_page() {
        let dir = std::env::temp_dir()
            .join("md-emit-tests")
            .join(format!("export-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut d = doc();
        d.pages.push(Page::new(
            md_doc::PageId::from_static("pg_about"),
            "About",
            "about",
            1440.0,
            900.0,
        ));

        let report = export(&d, &dir, &ExportOptions::default()).unwrap();
        assert!(dir.join("index.html").exists());
        assert!(dir.join("about/index.html").exists());
        assert_eq!(
            report
                .files
                .iter()
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("html"))
                .count(),
            2
        );
        assert!(report.bytes > 0);
    }

    #[test]
    fn a_page_ships_the_font_it_names() {
        let dir = std::env::temp_dir()
            .join("md-emit-tests")
            .join(format!("fonts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let report = export(&doc(), &dir, &ExportOptions::default()).unwrap();

        let shipped: Vec<_> = report
            .files
            .iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("woff2"))
            .collect();
        assert!(!shipped.is_empty(), "no font file was written");
        assert!(shipped.iter().all(|p| p.is_file()));

        let html = fs::read_to_string(dir.join("index.html")).unwrap();
        assert!(html.contains("@font-face"), "no rule pointed at it");
        assert!(
            html.contains(&format!("{}/", fonts::FONTS_DIR)),
            "the rule did not reference the fonts directory: {html}"
        );
    }

    #[test]
    fn font_embedding_can_be_turned_off() {
        let dir = std::env::temp_dir()
            .join("md-emit-tests")
            .join(format!("nofonts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let report = export(
            &doc(),
            &dir,
            &ExportOptions {
                embed_fonts: false,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(!report
            .files
            .iter()
            .any(|p| p.extension().and_then(|e| e.to_str()) == Some("woff2")));
        assert!(!fs::read_to_string(dir.join("index.html"))
            .unwrap()
            .contains("@font-face"));
    }
}
