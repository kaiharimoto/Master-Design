//! Rasterizing a page so the model can look at it.
//!
//! `doc.describe` tells a model what the document *contains*. That is not the same as
//! knowing what it *looks like* — nothing in a node list reveals that two elements
//! overlap, that the contrast is unreadable, or that an entrance animation starts
//! off-screen. A picture closes that loop, and closing it is most of why this server
//! exists.
//!
//! Rendering happens in-process through resvg, so a screenshot needs no browser, no
//! display server and no studio running.

use md_doc::{Document, Page};
use md_emit::frame;
use resvg::tiny_skia;
use resvg::usvg;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct SnapshotOptions {
    /// Output width in pixels. Height follows the page's aspect ratio.
    pub width: u32,
    /// Seconds into the page's animations. `None` renders the resting state.
    pub time: Option<f64>,
    /// Crop to a node's bounds, with a little padding, instead of the whole page.
    pub node: Option<md_doc::NodeId>,
    /// Explicit crop rectangle in document units, `[x, y, w, h]`.
    pub region: Option<[f64; 4]>,
    /// Solve the layout at this document width before rendering.
    ///
    /// This is how a model sees the phone layout rather than the desktop one shrunk down.
    /// `None` renders the design as authored, which is what "show me what I am editing"
    /// means.
    pub at_width: Option<f64>,
    /// The project directory, so images can be drawn.
    ///
    /// Without it a design with photographs in it rasterizes as white boxes — which is
    /// worse than useless for a model checking its own work, because the picture *looks*
    /// like a finished render.
    pub assets_from: Option<std::path::PathBuf>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        // 1024 is wide enough to judge layout and type, small enough that the base64
        // payload does not dominate the model's context.
        SnapshotOptions {
            width: 1024,
            time: None,
            node: None,
            region: None,
            at_width: None,
            assets_from: None,
        }
    }
}

/// The font database the rasterizer draws with.
///
/// Deliberately the *same* one `md-text` measured with. A snapshot rendered from a
/// different set of fonts is a picture of a document that does not exist: the layout the
/// model is looking at would be one the editor and the exported page never produce. It
/// also means Inter is always present, which a bare system scan cannot promise — a CI
/// runner or an Android device may not have it.
///
/// Cloned rather than borrowed because `usvg::Options` wants an `Arc` of its own; the
/// clone happens once, on first use.
fn fonts() -> &'static usvg::fontdb::Database {
    static DB: OnceLock<usvg::fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| md_text::shared().db().clone())
}

/// Render a page to PNG bytes.
pub fn snapshot(
    doc: &Document,
    page_key: &str,
    opts: &SnapshotOptions,
) -> Result<(Vec<u8>, u32, u32), String> {
    let staged;
    let source = match opts.time {
        Some(t) => {
            staged = frame::at_time(doc, page_key, t);
            &staged
        }
        None => doc,
    };

    let authored: &Page = source
        .page(page_key)
        .ok_or_else(|| format!("no page '{page_key}' in this document"))?;

    // Solving produces a page, so it has to be owned; borrowing the authored one when no
    // width was asked for keeps the common path free of a clone of the whole scene graph.
    let solved;
    let page: &Page = match opts.at_width {
        Some(width) if width > 0.0 => {
            solved = md_layout::solve(authored, width);
            &solved
        }
        _ => authored,
    };

    let crop = resolve_crop(source, page, opts)?;
    let mut svg = wrap_for_crop(source, page, crop);
    if let Some(project) = &opts.assets_from {
        svg = inline_assets(&svg, project);
    }

    let mut options = usvg::Options {
        fontdb: std::sync::Arc::new(fonts().clone()),
        ..Default::default()
    };
    // Left unset on purpose. Images are inlined above, by code that checks each name
    // against the project's assets folder; handing resvg a base directory instead would
    // let a document containing `assets/../../.ssh/id_rsa` decide what gets read, and a
    // document may have been written by a model or edited by hand.
    options.resources_dir = None;

    let tree = usvg::Tree::from_str(&svg, &options)
        .map_err(|e| format!("could not parse the rendered SVG: {e}"))?;

    let width = opts.width.clamp(16, 4096);
    let scale = width as f64 / crop[2].max(1.0);
    let height = ((crop[3] * scale).round() as u32).clamp(16, 8192);

    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("could not allocate a {width}×{height} image"))?;

    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );

    let png = pixmap
        .encode_png()
        .map_err(|e| format!("could not encode PNG: {e}"))?;
    Ok((png, width, height))
}

/// Work out what area to render.
fn resolve_crop(doc: &Document, page: &Page, opts: &SnapshotOptions) -> Result<[f64; 4], String> {
    if let Some(region) = opts.region {
        if region[2] <= 0.0 || region[3] <= 0.0 {
            return Err("region must have a positive width and height".to_string());
        }
        return Ok(region);
    }

    if let Some(id) = &opts.node {
        let node = doc
            .node(id)
            .ok_or_else(|| format!("no node {id} in this document"))?;
        let bounds = node
            .bounds_in_parent()
            .ok_or_else(|| format!("{} has no measurable bounds", node.display_name()))?;

        // A tight crop on a shape is usually unhelpful — context is what makes a
        // screenshot legible — so pad by a fraction of the larger dimension.
        let pad = (bounds.w.max(bounds.h) * 0.15).max(16.0);
        return Ok([
            bounds.x - pad,
            bounds.y - pad,
            (bounds.w + pad * 2.0).max(1.0),
            (bounds.h + pad * 2.0).max(1.0),
        ]);
    }

    Ok([0.0, 0.0, page.width, page.height])
}

/// Re-render the page with a viewBox matching the crop.
///
/// The exporter always emits a full-page viewBox, so cropping means rewriting that one
/// attribute rather than teaching the exporter about regions it will never need
/// anywhere else.
///
/// `render_svg(.., true)` rather than the HTML: there is no stylesheet here, so the page
/// colour has to be painted into the artwork, and asking for the SVG directly means the
/// crop never depends on finding `</svg>` inside a document that also contains scripts.
fn wrap_for_crop(doc: &Document, page: &Page, crop: [f64; 4]) -> String {
    let (svg, _) = md_emit::render_svg(doc, page, true);

    let full = format!("viewBox=\"0 0 {} {}\"", num(page.width), num(page.height));
    let cropped = format!(
        "viewBox=\"{} {} {} {}\"",
        num(crop[0]),
        num(crop[1]),
        num(crop[2]),
        num(crop[3])
    );

    let mut out = svg.replace(&full, &cropped);

    // A page with no background is transparent, which reads as black in most PNG
    // viewers and makes a design look wrong for reasons that are not its fault.
    if page.background.is_none() {
        let backdrop = format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>",
            num(crop[0]),
            num(crop[1]),
            num(crop[2]),
            num(crop[3])
        );
        if let Some(pos) = out.find('>') {
            out.insert_str(pos + 1, &backdrop);
        }
    }

    out
}

/// Replace `href="assets/NAME"` with the file's bytes as a data URI.
///
/// Inlining rather than pointing resvg at a directory, so that the decision about which
/// files may be read stays in code that can refuse: `md_doc::assets::path_of` rejects any
/// name that would climb out of the assets folder, and a name it rejects is simply left
/// as it was — the element renders empty rather than the whole snapshot failing.
fn inline_assets(svg: &str, project: &std::path::Path) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    const NEEDLE: &str = "href=\"assets/";

    while let Some(start) = rest.find(NEEDLE) {
        let after = start + NEEDLE.len();
        let Some(end) = rest[after..].find('"') else {
            break;
        };
        let name = &rest[after..after + end];

        out.push_str(&rest[..start]);
        match read_asset(project, name) {
            Some(uri) => {
                out.push_str("href=\"");
                out.push_str(&uri);
                out.push('"');
            }
            None => {
                // Keep the original reference. It will not resolve, which is exactly what
                // should happen to an asset that is missing or out of bounds.
                out.push_str(&rest[start..after + end + 1]);
            }
        }
        rest = &rest[after + end + 1..];
    }
    out.push_str(rest);
    out
}

fn read_asset(project: &std::path::Path, name: &str) -> Option<String> {
    use base64::Engine;

    let path = md_doc::assets::path_of(project, name).ok()?;
    let bytes = std::fs::read(&path).ok()?;
    let mime = match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

fn num(v: f64) -> String {
    md_geom::fmt_coord(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::node::{NodeKind, RectGeometry};
    use md_doc::{Node, NodeId, Paint};

    fn doc() -> Document {
        let mut d = Document::new("Snapshot test");
        d.pages[0].root.id = NodeId::from_static("nd_root");
        d.pages[0].background = Some(Paint::solid("#101820").unwrap());
        d.pages[0].root.children.push(
            Node::new(
                NodeId::from_static("nd_card"),
                NodeKind::Rect(RectGeometry {
                    width: 400.0,
                    height: 300.0,
                    corner_radius: [24.0; 4],
                }),
            )
            .with_fill(Paint::solid("#ff0055").unwrap()),
        );
        d
    }

    fn decode(png: &[u8]) -> (u32, u32) {
        // PNG width and height live at bytes 16..24 of the IHDR chunk.
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        (w, h)
    }

    /// The rasterizer must be able to draw the family the tool measures with, or a
    /// snapshot is a picture of a document nobody will ever see.
    ///
    /// This is also the guard on a subtler failure: `md-text` and `resvg` share one
    /// `fontdb`, and if a dependency bump ever split them into two versions the database
    /// handed over here would be a different type — or worse, the same type from a
    /// different crate, and the faces would silently not be found.
    #[test]
    fn the_rasterizer_has_the_font_the_measurements_used() {
        let db = fonts();
        assert!(db
            .query(&usvg::fontdb::Query {
                families: &[usvg::fontdb::Family::Name("Inter")],
                weight: usvg::fontdb::Weight(400),
                stretch: usvg::fontdb::Stretch::Normal,
                style: usvg::fontdb::Style::Normal,
            })
            .is_some());
    }

    #[test]
    fn text_reaches_the_pixels() {
        use md_doc::text::TextGeometry;

        let mut with_text = doc();
        with_text.pages[0].root.children.push(
            Node::new(
                NodeId::from_static("nd_words"),
                NodeKind::Text(TextGeometry::new("HELLO", "Inter", 200.0)),
            )
            .with_fill(Paint::solid("#ffffff").unwrap()),
        );

        let plain = snapshot(&doc(), "index", &SnapshotOptions::default())
            .unwrap()
            .0;
        let lettered = snapshot(&with_text, "index", &SnapshotOptions::default())
            .unwrap()
            .0;

        // If the font were missing, resvg would draw nothing and the two images would be
        // identical — which is exactly the failure this catches.
        assert_ne!(
            plain, lettered,
            "adding 200px of text changed no pixels, so no font was found"
        );
    }

    #[test]
    fn a_page_renders_to_a_real_png() {
        let (png, w, h) = snapshot(&doc(), "index", &SnapshotOptions::default()).unwrap();
        assert_eq!(&png[1..4], b"PNG", "not a PNG");
        assert_eq!((w, h), decode(&png));
        // 1440×900 scaled to 1024 wide.
        assert_eq!(w, 1024);
        assert_eq!(h, 640);
    }

    #[test]
    fn cropping_to_a_node_follows_that_node_s_aspect_ratio() {
        let opts = SnapshotOptions {
            node: Some(NodeId::from_static("nd_card")),
            width: 400,
            ..Default::default()
        };
        let (_, w, h) = snapshot(&doc(), "index", &opts).unwrap();
        assert_eq!(w, 400);
        // 400×300 padded by 60 each way → 520×420, so height is 400 × 420/520.
        assert!((h as i64 - 323).abs() <= 2, "got {h}");
    }

    #[test]
    fn an_explicit_region_is_honoured() {
        let opts = SnapshotOptions {
            region: Some([0.0, 0.0, 200.0, 200.0]),
            width: 256,
            ..Default::default()
        };
        let (_, w, h) = snapshot(&doc(), "index", &opts).unwrap();
        assert_eq!((w, h), (256, 256));
    }

    #[test]
    fn a_zero_sized_region_is_refused() {
        let opts = SnapshotOptions {
            region: Some([0.0, 0.0, 0.0, 10.0]),
            ..Default::default()
        };
        assert!(snapshot(&doc(), "index", &opts).is_err());
    }

    #[test]
    fn rendering_at_a_width_shows_the_layout_that_width_gets() {
        // The point of `atWidth`: a model asking "how does this look on a phone" gets the
        // phone *layout*, not the desktop design shrunk to phone size.
        use md_doc::node::{Constraints, HConstraint, VConstraint};

        let mut d = doc();
        let mut band = Node::new(
            NodeId::from_static("nd_band"),
            NodeKind::Rect(RectGeometry {
                width: 1340.0,
                height: 100.0,
                corner_radius: [0.0; 4],
            }),
        )
        .with_fill(Paint::solid("#6d5efc").unwrap());
        band.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        d.pages[0].root.children.push(band);
        let (page_w, page_h) = (d.pages[0].width, d.pages[0].height);
        if let md_doc::NodeKind::Frame(f) = &mut d.pages[0].root.kind {
            f.width = page_w;
            f.height = page_h;
        }

        let authored = snapshot(&d, "index", &SnapshotOptions::default()).unwrap();
        let phone = snapshot(
            &d,
            "index",
            &SnapshotOptions {
                at_width: Some(390.0),
                ..Default::default()
            },
        )
        .unwrap();

        assert_ne!(authored.0, phone.0, "the solve changed nothing");
        // 1440×900 → 390×900: much taller relative to its width.
        assert!(
            phone.2 > authored.2,
            "the phone render should be proportionally taller: {} vs {}",
            phone.2,
            authored.2
        );
    }

    #[test]
    fn a_width_of_zero_falls_back_to_the_design() {
        let opts = SnapshotOptions {
            at_width: Some(0.0),
            ..Default::default()
        };
        let solved = snapshot(&doc(), "index", &opts).unwrap();
        let authored = snapshot(&doc(), "index", &SnapshotOptions::default()).unwrap();
        assert_eq!(solved.0, authored.0);
    }

    #[test]
    fn an_unknown_page_is_an_error() {
        assert!(snapshot(&doc(), "nope", &SnapshotOptions::default()).is_err());
    }

    /// A real 2×2 PNG: solid red, so its pixels are unmistakable in a render.
    fn red_png() -> Vec<u8> {
        // Built rather than embedded so the test carries its own explanation.
        fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            let body: Vec<u8> = kind.iter().chain(data).copied().collect();
            out.extend(&body);
            out.extend(crc32(&body).to_be_bytes());
            out
        }
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for byte in data {
                crc ^= *byte as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = 2u32.to_be_bytes().to_vec();
        ihdr.extend(2u32.to_be_bytes());
        ihdr.extend([8, 2, 0, 0, 0]); // 8-bit RGB
        png.extend(chunk(b"IHDR", &ihdr));

        // Two rows, each a filter byte then two red pixels, deflated with stored blocks.
        let raw: Vec<u8> = vec![0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0, 0];
        let mut z = vec![0x78, 0x01];
        z.push(0x01);
        z.extend((raw.len() as u16).to_le_bytes());
        z.extend((!(raw.len() as u16)).to_le_bytes());
        z.extend(&raw);
        let mut adler_a: u32 = 1;
        let mut adler_b: u32 = 0;
        for byte in &raw {
            adler_a = (adler_a + *byte as u32) % 65521;
            adler_b = (adler_b + adler_a) % 65521;
        }
        z.extend(((adler_b << 16) | adler_a).to_be_bytes());
        png.extend(chunk(b"IDAT", &z));
        png.extend(chunk(b"IEND", b""));
        png
    }

    #[test]
    fn an_image_reaches_the_pixels_when_the_project_is_known() {
        // Without this a model checking its own work sees white boxes where the
        // photographs are, which is worse than useless: it looks like a finished render.
        use md_doc::node::ImageGeometry;

        let dir = std::env::temp_dir()
            .join("md-mcp-assets")
            .join(format!("render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let asset = md_doc::assets::import_bytes(&dir, "red.png", &red_png()).unwrap();

        let mut d = doc();
        d.pages[0].root.children.push(Node::new(
            NodeId::from_static("nd_img"),
            NodeKind::Image(ImageGeometry {
                asset: asset.name.clone(),
                width: 400.0,
                height: 400.0,
                fit: Default::default(),
            }),
        ));

        let without = snapshot(&d, "index", &SnapshotOptions::default())
            .unwrap()
            .0;
        let with = snapshot(
            &d,
            "index",
            &SnapshotOptions {
                assets_from: Some(dir.clone()),
                ..Default::default()
            },
        )
        .unwrap()
        .0;

        assert_ne!(
            without, with,
            "naming the project changed nothing, so the image was not drawn"
        );
    }

    #[test]
    fn an_asset_reference_that_climbs_out_of_the_project_is_not_followed() {
        let dir = std::env::temp_dir()
            .join("md-mcp-assets")
            .join(format!("escape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("secret.png");
        std::fs::write(&secret, red_png()).unwrap();

        let svg = "<svg><image href=\"assets/../secret.png\"/></svg>";
        let out = inline_assets(svg, &dir);
        assert_eq!(out, svg, "a traversing reference was resolved: {out}");
        assert!(!out.contains("data:"));
    }

    #[test]
    fn a_missing_asset_leaves_the_rest_of_the_page_alone() {
        let dir = std::env::temp_dir().join("md-mcp-assets").join("absent");
        let svg = "<svg><rect/><image href=\"assets/nope.png\"/><rect/></svg>";
        assert_eq!(inline_assets(svg, &dir), svg);
    }

    #[test]
    fn several_images_are_all_inlined() {
        let dir = std::env::temp_dir()
            .join("md-mcp-assets")
            .join(format!("many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = md_doc::assets::import_bytes(&dir, "a.png", &red_png()).unwrap();
        let mut other = red_png();
        other.push(0);
        let b = md_doc::assets::import_bytes(&dir, "b.png", &other).unwrap();

        let svg = format!(
            "<image href=\"assets/{}\"/><image href=\"assets/{}\"/>",
            a.name, b.name
        );
        let out = inline_assets(&svg, &dir);
        assert_eq!(
            out.matches("data:image/png;base64,").count(),
            2,
            "got {out}"
        );
    }

    #[test]
    fn a_transparent_page_gets_an_opaque_backdrop() {
        let mut d = doc();
        d.pages[0].background = None;
        let (png, _, _) = snapshot(&d, "index", &SnapshotOptions::default()).unwrap();
        assert!(png.len() > 100);
    }

    #[test]
    fn snapshots_at_different_times_differ() {
        use md_doc::anim::{Keyframe, Timeline, Track, Trigger};
        use md_doc::{Easing, TimelineId};
        use serde_json::json;

        let mut d = doc();
        d.pages[0].timelines.push(Timeline {
            id: TimelineId::from_static("tl_in"),
            name: "In".into(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: true,
            reduced_motion: Default::default(),
            source: None,
            tracks: vec![Track {
                target: NodeId::from_static("nd_card"),
                property: "translateX".into(),
                keyframes: vec![
                    Keyframe {
                        t: 0.0,
                        value: json!(0),
                        easing: Easing::Linear,
                    },
                    Keyframe {
                        t: 1.0,
                        value: json!(600),
                        easing: Easing::Linear,
                    },
                ],
            }],
        });

        let start = snapshot(
            &d,
            "index",
            &SnapshotOptions {
                time: Some(0.0),
                ..Default::default()
            },
        )
        .unwrap()
        .0;
        let end = snapshot(
            &d,
            "index",
            &SnapshotOptions {
                time: Some(1.0),
                ..Default::default()
            },
        )
        .unwrap()
        .0;

        assert_ne!(
            start, end,
            "the animation did not move between the two frames"
        );
    }
}
