//! Rendering the scene graph to SVG.
//!
//! SVG rather than positioned HTML boxes, because the thing being designed is *graphic*
//! — arbitrary vector artwork, not a stack of rectangles. Emitting the same primitives
//! the editor draws with means what ships is what was designed, rather than an
//! approximation reconstructed in a second layout model.
//!
//! What that costs is addressed elsewhere: text stays as real `<text>` so it is
//! selectable, and `html::accessibility_outline` emits a parallel semantic document so
//! screen readers and search engines get structure rather than a picture.

use md_doc::node::{Node, NodeKind};
use md_doc::paint::{Effect, Paint, StrokeAlign};
use md_doc::{Color, Page, Stroke};
use std::collections::BTreeSet;
use std::fmt::Write;

/// Accumulates markup plus the `<defs>` entries it referred to.
pub struct SvgWriter<'a> {
    defs: String,
    counter: usize,
    /// Nodes whose stroke must carry a dash array so a draw-on animation has something
    /// to offset. Computed from the timelines before rendering starts.
    pub dashed: &'a BTreeSet<String>,
    /// Nodes any timeline animates.
    pub animated: &'a BTreeSet<String>,
    pub warnings: Vec<String>,
}

impl<'a> SvgWriter<'a> {
    pub fn new(dashed: &'a BTreeSet<String>, animated: &'a BTreeSet<String>) -> Self {
        SvgWriter {
            defs: String::new(),
            counter: 0,
            dashed,
            animated,
            warnings: Vec::new(),
        }
    }

    /// Render a whole page, returning the `<svg>` element.
    ///
    /// `paint_background` decides who owns the page colour. In an exported page the
    /// stylesheet owns it, because the body extends past the artwork and both painting
    /// it would composite a translucent colour twice — visible as a seam along the
    /// bottom edge of the canvas. A rasterizer has no stylesheet, so it asks for the
    /// rect instead.
    pub fn page(&mut self, page: &Page, paint_background: bool) -> String {
        let body = self.node(&page.root);

        let background = page
            .background
            .as_ref()
            .filter(|_| paint_background)
            .map(|p| {
                let paint = self.paint_value(p, "page");
                format!(
                    "<rect width=\"{}\" height=\"{}\" fill=\"{}\"{}/>",
                    num(page.width),
                    num(page.height),
                    paint,
                    opacity_attr("fill-opacity", p)
                )
            })
            .unwrap_or_default();

        let defs = if self.defs.is_empty() {
            String::new()
        } else {
            format!("<defs>{}</defs>", self.defs)
        };

        format!(
            "<svg class=\"md-canvas\" viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\" \
             preserveAspectRatio=\"xMidYMid meet\" role=\"presentation\" aria-hidden=\"true\">\
             {defs}{background}{body}</svg>",
            num(page.width),
            num(page.height)
        )
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("md-{prefix}-{}", self.counter)
    }

    /// Render one node and its subtree.
    pub fn node(&mut self, node: &Node) -> String {
        if !node.visible {
            return String::new();
        }

        let inner = match &node.kind {
            NodeKind::Group | NodeKind::Frame(_) => {
                let children: String = node
                    .children
                    .iter()
                    .map(|c| self.node(c))
                    .collect::<Vec<_>>()
                    .join("");
                match &node.kind {
                    NodeKind::Frame(f) => {
                        let shape =
                            md_geom::rect_path(0.0, 0.0, f.width, f.height, f.corner_radius);
                        if shape.is_empty() {
                            self.warn_no_area(node);
                        }

                        // A frame's own fill is a rectangle behind its children — and so
                        // is its border, which a fills-only test used to throw away.
                        let mut out = String::new();
                        if !shape.is_empty() && (!node.fills.is_empty() || !node.strokes.is_empty())
                        {
                            out.push_str(&self.shape_with_paints(node, &shape, ""));
                        }
                        // Clipping to an empty region would delete the children too, so a
                        // frame with no area keeps them rather than compounding the loss.
                        if f.clip && !shape.is_empty() {
                            let clip_id = self.next_id("clip");
                            let _ = write!(
                                self.defs,
                                "<clipPath id=\"{clip_id}\"><path d=\"{}\"/></clipPath>",
                                esc_attr(&shape)
                            );
                            let _ = write!(out, "<g clip-path=\"url(#{clip_id})\">{children}</g>");
                        } else {
                            out.push_str(&children);
                        }
                        out
                    }
                    _ => children,
                }
            }
            NodeKind::Text(t) => self.text(node, t),
            NodeKind::Image(i) => {
                use md_doc::paint::ImageFit;
                let shape = node.geometry_path().unwrap_or_default();
                if shape.is_empty() {
                    // Every renderer drops a zero-sized `<image>`, so it goes the same
                    // way as any other geometry with no area rather than shipping as an
                    // element that looks present in the file and is not on the screen.
                    self.warn_no_area(node);
                    return String::new();
                }

                if i.fit == ImageFit::Tile {
                    self.warnings.push(format!(
                        "{}: tiled images are not implemented; it was rendered as cover",
                        node.display_name()
                    ));
                }

                let href = format!("assets/{}", i.asset);
                let mut out = format!(
                    "<image href=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"{}\"/>",
                    esc_attr(&href),
                    num(i.width),
                    num(i.height),
                    match i.fit {
                        ImageFit::Cover | ImageFit::Tile => "xMidYMid slice",
                        ImageFit::Contain => "xMidYMid meet",
                        ImageFit::Fill => "none",
                    }
                );

                // A stroke on an image is its border and belongs over the picture. Its
                // fills deliberately do not: painting a colour over a photograph hides
                // the thing the node exists to show.
                for (index, stroke) in node.strokes.iter().enumerate() {
                    out.push_str(&self.stroke_path(node, &shape, stroke, index, ""));
                }
                out
            }
            _ => {
                let shape = node.geometry_path().unwrap_or_default();
                if shape.is_empty() {
                    self.warn_no_area(node);
                }
                let extra = match &node.kind {
                    NodeKind::Path(p) if p.fill_rule == md_geom::FillRule::EvenOdd => {
                        " fill-rule=\"evenodd\""
                    }
                    _ => "",
                };
                self.shape_with_paints(node, &shape, extra)
            }
        };

        if inner.is_empty() {
            return String::new();
        }
        self.wrap(node, inner)
    }

    /// Wrap a node's content in a `<g>` carrying its transform, opacity and effects.
    ///
    /// Always a group, even when nothing needs wrapping: the animation runtime looks up
    /// elements by `data-md-id`, and a stable one-element-per-node mapping is worth more
    /// than the handful of bytes a smarter emitter would save.
    fn wrap(&mut self, node: &Node, inner: String) -> String {
        let mut attrs = format!(" data-md-id=\"{}\"", esc_attr(node.id.as_str()));

        if !node.name.is_empty() {
            let _ = write!(attrs, " data-md-name=\"{}\"", esc_attr(&node.name));
        }
        if let Some(primary) = node.roles.first() {
            // The first role gets its own attribute, for selectors that want exactly one;
            // all of them land in `class`, so exported and hand-written CSS can reach any.
            let _ = write!(attrs, " data-md-role=\"{}\"", esc_attr(primary));
            let _ = write!(attrs, " class=\"{}\"", esc_attr(&node.roles.join(" ")));
        }

        if let Some(t) = node.transform.to_svg_attr() {
            let _ = write!(attrs, " transform=\"{}\"", esc_attr(&t));
        }
        if node.opacity < 1.0 {
            let _ = write!(attrs, " opacity=\"{}\"", num(node.opacity));
        }
        if node.blend_mode != md_doc::BlendMode::Normal {
            let _ = write!(
                attrs,
                " style=\"mix-blend-mode:{}\"",
                node.blend_mode.as_css()
            );
        }
        if !node.effects.is_empty() {
            let id = self.filter(&node.effects);
            let _ = write!(attrs, " filter=\"url(#{id})\"");
        }

        // Animated nodes get a second, inner group to move in.
        //
        // SVG's `transform` attribute is the CSS `transform` property wearing a
        // different hat, so an animation that sets `transform` on this element would
        // replace the node's placement matrix and teleport it to the origin. Giving the
        // animation its own element to own outright avoids the collision entirely.
        let inner = if self.animated.contains(node.id.as_str()) {
            format!(
                "<g data-md-fx=\"{}\">{inner}</g>",
                esc_attr(node.id.as_str())
            )
        } else {
            inner
        };

        format!("<g{attrs}>{inner}</g>")
    }

    /// Emit a shape, duplicating it once per paint when a node carries more than one.
    ///
    /// SVG allows a single `fill` and a single `stroke` per element. Designers routinely
    /// stack a gradient over a base colour, so rather than dropping the extras we draw
    /// the geometry again per layer.
    fn shape_with_paints(&mut self, node: &Node, d: &str, extra: &str) -> String {
        if d.is_empty() {
            return String::new();
        }
        let esc = esc_attr(d);
        let mut out = String::new();

        if node.fills.is_empty() && node.strokes.is_empty() {
            return format!("<path d=\"{esc}\" fill=\"none\"{extra}/>");
        }

        for (i, fill) in node.fills.iter().enumerate() {
            let paint = self.paint_value(fill, &format!("{}-f{i}", node.id));
            let op_attr = opacity_attr("fill-opacity", fill);
            let _ = write!(out, "<path d=\"{esc}\" fill=\"{paint}\"{op_attr}{extra}/>");
        }

        for (i, stroke) in node.strokes.iter().enumerate() {
            let _ = write!(out, "{}", self.stroke_path(node, d, stroke, i, extra));
        }

        out
    }

    fn stroke_path(
        &mut self,
        node: &Node,
        d: &str,
        stroke: &Stroke,
        index: usize,
        extra: &str,
    ) -> String {
        // Inside and outside strokes have no SVG equivalent. Outlining converts them to
        // a filled region, which is exact rather than approximated by doubling the width.
        if stroke.align != StrokeAlign::Center {
            match md_geom::outline_stroke(d, &stroke.style()) {
                Ok(outlined) => {
                    let paint = self.paint_value(&stroke.paint, &format!("{}-s{index}", node.id));
                    let op_attr = opacity_attr("fill-opacity", &stroke.paint);
                    return format!(
                        "<path d=\"{}\" fill=\"{paint}\"{op_attr}/>",
                        esc_attr(&outlined)
                    );
                }
                Err(e) => self.warnings.push(format!(
                    "{}: could not outline a {:?}-aligned stroke ({e}); drew it centred",
                    node.display_name(),
                    stroke.align
                )),
            }
        }

        let paint = self.paint_value(&stroke.paint, &format!("{}-s{index}", node.id));
        let mut attrs = format!(
            " stroke=\"{paint}\" stroke-width=\"{}\"{} fill=\"none\"",
            num(stroke.width),
            opacity_attr("stroke-opacity", &stroke.paint)
        );

        let cap = match stroke.cap {
            md_geom::LineCap::Butt => "butt",
            md_geom::LineCap::Round => "round",
            md_geom::LineCap::Square => "square",
        };
        let join = match stroke.join {
            md_geom::LineJoin::Miter => "miter",
            md_geom::LineJoin::Round => "round",
            md_geom::LineJoin::Bevel => "bevel",
        };
        if cap != "butt" {
            let _ = write!(attrs, " stroke-linecap=\"{cap}\"");
        }
        if join != "miter" {
            let _ = write!(attrs, " stroke-linejoin=\"{join}\"");
        }

        // A draw-on animation offsets a dash pattern, so the pattern has to exist and be
        // exactly as long as the path. Without this the stroke is simply solid and the
        // animation does nothing visible.
        if self.dashed.contains(node.id.as_str()) && stroke.dash.is_empty() {
            if let Ok(len) = md_geom::path_length(d) {
                let _ = write!(attrs, " stroke-dasharray=\"{}\"", num(len));
            }
        } else if !stroke.dash.is_empty() {
            let dashes: Vec<String> = stroke.dash.iter().map(|v| num(*v)).collect();
            let _ = write!(attrs, " stroke-dasharray=\"{}\"", dashes.join(" "));
            if stroke.dash_offset != 0.0 {
                let _ = write!(attrs, " stroke-dashoffset=\"{}\"", num(stroke.dash_offset));
            }
        }

        format!("<path d=\"{}\"{attrs}{extra}/>", esc_attr(d))
    }

    /// Draw a text node.
    ///
    /// Every position here comes from `md-text`, which shapes the run through the same
    /// HarfBuzz algorithm the browser will use. Before that, baselines were placed at a
    /// flat `0.8em` and wrapping did not happen at all, so a text box with a measure set
    /// exported as one long line running out of the page.
    fn text(&mut self, node: &Node, t: &md_doc::TextGeometry) -> String {
        let f = &t.font;
        let anchor = t.align.text_anchor();
        let layout = t.layout();

        if layout.substituted {
            self.warnings.push(format!(
                "{}: no font matching \"{}\" {} was available, so it was measured and \
                 exported in a substitute — text may not sit where it does in the editor",
                node.display_name(),
                f.font_family,
                f.font_weight
            ));
        }
        if !t.spans.is_empty() {
            // Honest about a gap rather than dropping the overrides in silence.
            self.warnings.push(format!(
                "{}: character-range styling is not exported yet, so the whole block uses \
                 the base style",
                node.display_name()
            ));
        }

        // The box the text is aligned within: what the designer set, or what it measured.
        let box_width = t.width.unwrap_or(layout.width);
        let x = match t.align {
            md_doc::TextAlign::Center => box_width / 2.0,
            md_doc::TextAlign::Right => box_width,
            _ => 0.0,
        };

        // Vertical alignment only means anything inside a box with a stated height.
        let slack = t
            .height
            .map(|h| (h - layout.height).max(0.0))
            .unwrap_or(0.0);
        let dy = match t.vertical_align {
            md_doc::text::VerticalAlign::Top => 0.0,
            md_doc::text::VerticalAlign::Middle => slack / 2.0,
            md_doc::text::VerticalAlign::Bottom => slack,
        };

        let mut attrs = format!(
            " font-family=\"{}\" font-size=\"{}\"",
            esc_attr(&f.font_family),
            num(f.font_size)
        );
        if f.font_weight != 400 {
            let _ = write!(attrs, " font-weight=\"{}\"", f.font_weight);
        }
        if f.italic {
            let _ = write!(attrs, " font-style=\"italic\"");
        }
        if f.letter_spacing != 0.0 {
            let _ = write!(
                attrs,
                " letter-spacing=\"{}\"",
                num(f.letter_spacing * f.font_size)
            );
        }
        if anchor != "start" {
            let _ = write!(attrs, " text-anchor=\"{anchor}\"");
        }
        match f.decoration {
            md_doc::text::TextDecoration::Underline => {
                let _ = write!(attrs, " text-decoration=\"underline\"");
            }
            md_doc::text::TextDecoration::Strikethrough => {
                let _ = write!(attrs, " text-decoration=\"line-through\"");
            }
            md_doc::text::TextDecoration::None => {}
        }

        let (fill, fill_opacity) = match node.fills.first() {
            Some(p) => (
                self.paint_value(p, &format!("{}-t", node.id)),
                opacity_attr("fill-opacity", p),
            ),
            // Unfilled text would be invisible, which is never what was meant.
            None => (Color::default().to_string(), String::new()),
        };
        let _ = write!(attrs, " fill=\"{fill}\"{fill_opacity}");

        // Outlined text is a real design choice — a hollow display heading, a knockout on
        // a photograph — and it used to be dropped on the floor without a word.
        if let Some(stroke) = node.strokes.first() {
            let paint = self.paint_value(&stroke.paint, &format!("{}-ts", node.id));
            let _ = write!(
                attrs,
                " stroke=\"{paint}\"{} stroke-width=\"{}\" paint-order=\"stroke\"",
                opacity_attr("stroke-opacity", &stroke.paint),
                num(stroke.width)
            );
        }

        // `text-transform` is baked into the emitted characters rather than left to CSS:
        // a rasterizer has no CSS engine, so a snapshot of an uppercased heading would
        // otherwise come back in mixed case and be measured at the wrong width.
        let lines: Vec<String> = layout
            .lines
            .iter()
            .map(|line| {
                format!(
                    "<tspan x=\"{}\" y=\"{}\">{}</tspan>",
                    num(x),
                    num(line.baseline + dy),
                    esc_text(&line.text)
                )
            })
            .collect();

        format!("<text{attrs}>{}</text>", lines.join(""))
    }

    /// Resolve a paint to an SVG paint value, registering a gradient in `<defs>` when
    /// one is needed.
    fn paint_value(&mut self, paint: &Paint, hint: &str) -> String {
        match paint {
            Paint::Solid { color, .. } => color.hex_rgb(),
            Paint::LinearGradient {
                from, to, stops, ..
            } => {
                let id = format!("grad-{}-{}", sanitize(hint), self.bump());
                let _ = write!(
                    self.defs,
                    "<linearGradient id=\"{id}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">{}</linearGradient>",
                    num(from[0]),
                    num(from[1]),
                    num(to[0]),
                    num(to[1]),
                    stops_markup(stops)
                );
                format!("url(#{id})")
            }
            Paint::RadialGradient {
                center,
                radius,
                stops,
                ..
            } => {
                let id = format!("grad-{}-{}", sanitize(hint), self.bump());
                let _ = write!(
                    self.defs,
                    "<radialGradient id=\"{id}\" cx=\"{}\" cy=\"{}\" r=\"{}\">{}</radialGradient>",
                    num(center[0]),
                    num(center[1]),
                    num(*radius),
                    stops_markup(stops)
                );
                format!("url(#{id})")
            }
            Paint::Image { asset, .. } => {
                let id = format!("pat-{}-{}", sanitize(hint), self.bump());
                let _ = write!(
                    self.defs,
                    "<pattern id=\"{id}\" patternContentUnits=\"objectBoundingBox\" width=\"1\" height=\"1\">\
                     <image href=\"assets/{}\" width=\"1\" height=\"1\" preserveAspectRatio=\"xMidYMid slice\"/></pattern>",
                    esc_attr(asset)
                );
                format!("url(#{id})")
            }
        }
    }

    /// Note geometry that baked to an empty path.
    ///
    /// Zero or negative extents produce no path at all, and once that is in the file an
    /// absent layer is indistinguishable from a deleted one. Naming it is the difference
    /// between a bug someone can find and a design that quietly lost a piece.
    fn warn_no_area(&mut self, node: &Node) {
        self.warnings.push(format!(
            "{}: no geometry to draw, usually a zero or negative size; it was left out",
            node.display_name()
        ));
    }

    fn filter(&mut self, effects: &[Effect]) -> String {
        let id = self.next_id("filter");
        let mut body = String::new();
        for (i, effect) in effects.iter().enumerate() {
            match effect {
                Effect::Blur { radius } => {
                    // SVG's stdDeviation is roughly half a CSS blur radius.
                    let _ = write!(
                        body,
                        "<feGaussianBlur stdDeviation=\"{}\"/>",
                        num(radius / 2.0)
                    );
                }
                Effect::DropShadow {
                    dx,
                    dy,
                    blur,
                    color,
                } => {
                    let _ = write!(
                        body,
                        "<feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-color=\"{}\" flood-opacity=\"{}\"/>",
                        num(*dx),
                        num(*dy),
                        num(blur / 2.0),
                        color.hex_rgb(),
                        num(color.alpha())
                    );
                }
                Effect::InnerShadow {
                    dx,
                    dy,
                    blur,
                    color,
                } => {
                    // SVG has no inner-shadow primitive, so it is assembled: offset and
                    // blur the source's alpha, subtract that from the alpha to leave the
                    // band just inside the edge, and flood the band with the colour.
                    //
                    // Every step names its input and its output. Implicit chaining reads
                    // as if the primitives compose in order when they do not, which is
                    // how the earlier version came to compute an outer region, discard
                    // it, and paint the node as a solid slab of the shadow colour.
                    let k = format!("inner{i}");
                    let _ = write!(
                        body,
                        "<feOffset in=\"SourceAlpha\" dx=\"{}\" dy=\"{}\" result=\"{k}-offset\"/>\
                         <feGaussianBlur in=\"{k}-offset\" stdDeviation=\"{}\" result=\"{k}-blur\"/>\
                         <feComposite in=\"SourceAlpha\" in2=\"{k}-blur\" operator=\"out\" result=\"{k}-band\"/>\
                         <feFlood flood-color=\"{}\" flood-opacity=\"{}\" result=\"{k}-colour\"/>\
                         <feComposite in=\"{k}-colour\" in2=\"{k}-band\" operator=\"in\" result=\"{k}-shadow\"/>\
                         <feComposite in=\"{k}-shadow\" in2=\"SourceGraphic\" operator=\"over\"/>",
                        num(*dx),
                        num(*dy),
                        num(blur / 2.0),
                        color.hex_rgb(),
                        num(color.alpha())
                    );
                }
            }
        }
        // Room for shadows and blurs to spill past the source bounds.
        let _ = write!(
            self.defs,
            "<filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">{body}</filter>"
        );
        id
    }

    fn bump(&mut self) -> usize {
        self.counter += 1;
        self.counter
    }
}

/// A paint's effective alpha.
///
/// Transparency can be spelled two ways — the paint's own `opacity`, and the last two
/// hex digits of a solid's colour — and SVG's `fill` attribute only takes `#rrggbb`, so
/// the two have to be multiplied into the separate opacity attribute or one of them is
/// silently discarded. Gradient stops carry their alpha in `stop-opacity` already, so
/// only the paint's own opacity applies there.
fn paint_alpha(paint: &Paint) -> f64 {
    match paint {
        Paint::Solid { color, opacity } => color.alpha() * opacity,
        _ => paint.opacity(),
    }
}

/// `fill-opacity` / `stroke-opacity`, or nothing at all when the paint is opaque.
fn opacity_attr(name: &str, paint: &Paint) -> String {
    let alpha = paint_alpha(paint);
    if alpha < 1.0 {
        format!(" {name}=\"{}\"", num(alpha))
    } else {
        String::new()
    }
}

fn stops_markup(stops: &[md_doc::GradientStop]) -> String {
    stops
        .iter()
        .map(|s| {
            let alpha = s.color.alpha();
            let op = if alpha < 1.0 {
                format!(" stop-opacity=\"{}\"", num(alpha))
            } else {
                String::new()
            };
            format!(
                "<stop offset=\"{}\" stop-color=\"{}\"{op}/>",
                num(s.offset),
                s.color.hex_rgb()
            )
        })
        .collect()
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

pub fn num(v: f64) -> String {
    md_geom::fmt_coord(v)
}

pub fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn esc_attr(s: &str) -> String {
    esc_text(s).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::node::{EllipseGeometry, FrameGeometry, ImageGeometry, RectGeometry};
    use md_doc::paint::{GradientStop, ImageFit};
    use md_doc::text::TextGeometry;
    use md_doc::{NodeId, PageId};

    fn page_with(node: Node) -> Page {
        let mut p = Page::new(PageId::from_static("pg_1"), "Home", "index", 100.0, 100.0);
        p.root.children.push(node);
        p
    }

    /// Writes the way a rasterizer asks for a page: background painted in, because
    /// there is no stylesheet behind it. The export path is covered separately by
    /// [`an_exported_page_leaves_the_background_to_css`].
    fn write(page: &Page) -> (String, Vec<String>) {
        let dashed = BTreeSet::new();
        let animated = BTreeSet::new();
        let mut w = SvgWriter::new(&dashed, &animated);
        let svg = w.page(page, true);
        (svg, w.warnings)
    }

    fn render(node: Node) -> String {
        write(&page_with(node)).0
    }

    fn warnings(node: Node) -> Vec<String> {
        write(&page_with(node)).1
    }

    /// Every value `name` takes in attribute position, in document order.
    ///
    /// The leading space matters: without it `in="` also matches the tail of
    /// `stroke-linejoin="`.
    fn attrs(svg: &str, name: &str) -> Vec<String> {
        let needle = format!(" {name}=\"");
        let mut out = Vec::new();
        let mut rest = svg;
        while let Some(i) = rest.find(&needle) {
            rest = &rest[i + needle.len()..];
            let Some(end) = rest.find('"') else { break };
            out.push(rest[..end].to_string());
            rest = &rest[end..];
        }
        out
    }

    fn attr(svg: &str, name: &str) -> Option<String> {
        attrs(svg, name).into_iter().next()
    }

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

    fn frame(w: f64, h: f64) -> Node {
        Node::new(
            NodeId::from_static("nd_f"),
            NodeKind::Frame(FrameGeometry {
                width: w,
                height: h,
                clip: false,
                corner_radius: [0.0; 4],
                layout: None,
            }),
        )
    }

    fn stop(offset: f64, hex: &str) -> GradientStop {
        GradientStop {
            offset,
            color: Color::parse(hex).unwrap(),
        }
    }

    #[test]
    fn a_colours_alpha_survives_as_fill_opacity() {
        let svg = render(rect(10.0, 10.0).with_fill(Paint::solid("#ff005580").unwrap()));
        assert!(svg.contains("fill=\"#ff0055\""), "got {svg}");
        assert_eq!(
            attr(&svg, "fill-opacity").as_deref(),
            Some("0.502"),
            "a translucent fill exported opaque: {svg}"
        );
    }

    #[test]
    fn a_paints_opacity_and_its_colours_alpha_multiply() {
        let svg = render(rect(10.0, 10.0).with_fill(Paint::Solid {
            color: Color::parse("#ff005580").unwrap(),
            opacity: 0.5,
        }));
        assert_eq!(
            attr(&svg, "fill-opacity").as_deref(),
            Some("0.251"),
            "one of the two alphas won outright: {svg}"
        );
    }

    #[test]
    fn a_strokes_alpha_survives_as_stroke_opacity() {
        let mut node = rect(10.0, 10.0);
        node.strokes
            .push(Stroke::new(Paint::solid("#00ff0040").unwrap(), 2.0));
        let svg = render(node);
        assert!(svg.contains("stroke=\"#00ff00\""), "got {svg}");
        assert_eq!(
            attr(&svg, "stroke-opacity").as_deref(),
            Some("0.251"),
            "a translucent stroke exported opaque: {svg}"
        );
    }

    #[test]
    fn an_outlined_stroke_keeps_its_alpha() {
        let mut node = rect(40.0, 40.0);
        let mut stroke = Stroke::new(Paint::solid("#00ff0080").unwrap(), 4.0);
        stroke.align = StrokeAlign::Inside;
        node.strokes.push(stroke);

        let (svg, warns) = write(&page_with(node));
        assert!(warns.is_empty(), "outlining failed: {warns:?}");
        assert_eq!(
            attr(&svg, "fill-opacity").as_deref(),
            Some("0.502"),
            "the outlined stroke exported opaque: {svg}"
        );
    }

    #[test]
    fn text_keeps_the_alpha_of_its_fill() {
        let node = Node::new(
            NodeId::from_static("nd_t"),
            NodeKind::Text(TextGeometry::new("Hello", "Inter", 16.0)),
        )
        .with_fill(Paint::solid("#11223380").unwrap());

        let svg = render(node);
        assert!(svg.contains("fill=\"#112233\""), "got {svg}");
        assert_eq!(
            attr(&svg, "fill-opacity").as_deref(),
            Some("0.502"),
            "translucent text exported opaque: {svg}"
        );
    }

    #[test]
    fn the_page_background_keeps_its_alpha() {
        let mut p = page_with(rect(10.0, 10.0));
        p.background = Some(Paint::solid("#0b102080").unwrap());
        let (svg, _) = write(&p);
        assert!(
            svg.contains(
                "<rect width=\"100\" height=\"100\" fill=\"#0b1020\" fill-opacity=\"0.502\"/>"
            ),
            "got {svg}"
        );
    }

    /// The regression this guards: an exported page painted its background twice —
    /// once in `body { background: … }` and once here — so a translucent page colour
    /// composited against itself and left a visible seam where the artwork ended.
    #[test]
    fn an_exported_page_leaves_the_background_to_css() {
        let mut p = page_with(rect(10.0, 10.0));
        p.background = Some(Paint::solid("#0b102080").unwrap());
        let dashed = BTreeSet::new();
        let animated = BTreeSet::new();
        let mut w = SvgWriter::new(&dashed, &animated);
        let svg = w.page(&p, false);
        assert!(
            !svg.contains("<rect width=\"100\" height=\"100\""),
            "the exported SVG painted the page background itself: {svg}"
        );
    }

    #[test]
    fn a_frame_with_only_a_stroke_still_draws_its_border() {
        let mut node = frame(80.0, 40.0);
        node.strokes
            .push(Stroke::new(Paint::solid("#ff0055").unwrap(), 2.0));
        let svg = render(node);
        assert!(
            svg.contains("stroke=\"#ff0055\""),
            "an unfilled bordered frame lost its border: {svg}"
        );
    }

    #[test]
    fn an_inner_shadow_wires_every_primitive_into_the_next() {
        let mut node = rect(40.0, 40.0).with_fill(Paint::solid("#ffffff").unwrap());
        node.effects.push(Effect::InnerShadow {
            dx: 0.0,
            dy: 2.0,
            blur: 6.0,
            color: Color::parse("#00000080").unwrap(),
        });
        let svg = render(node);

        let named = attrs(&svg, "result");
        assert!(!named.is_empty(), "the chain names nothing: {svg}");

        let mut consumed = attrs(&svg, "in");
        consumed.extend(attrs(&svg, "in2"));
        for r in &named {
            assert!(
                consumed.contains(r),
                "primitive result {r:?} is computed and then thrown away: {svg}"
            );
        }

        // The inner region is the source alpha minus its blurred copy. The other way
        // round is the region *outside* the shape, which is what made this a slab.
        assert!(
            svg.contains("in=\"SourceAlpha\" in2=\"inner0-blur\" operator=\"out\""),
            "got {svg}"
        );
        assert!(
            svg.contains("in2=\"SourceGraphic\" operator=\"over\""),
            "the shadow never lands on the artwork: {svg}"
        );
    }

    #[test]
    fn a_linear_gradient_is_defined_once_and_referenced() {
        let node = rect(10.0, 10.0).with_fill(Paint::LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 1.0],
            stops: vec![stop(0.0, "#ff0055"), stop(1.0, "#00000000")],
            opacity: 1.0,
        });
        let svg = render(node);

        let id = svg
            .split("<linearGradient id=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("no linear gradient in defs")
            .to_string();
        assert!(svg.contains(&format!("fill=\"url(#{id})\"")), "got {svg}");
        assert!(
            svg.contains("stop-opacity=\"0\""),
            "a fully transparent stop lost its alpha: {svg}"
        );
    }

    #[test]
    fn a_radial_gradient_carries_its_centre_and_radius() {
        let node = rect(10.0, 10.0).with_fill(Paint::RadialGradient {
            center: [0.5, 0.5],
            radius: 0.75,
            stops: vec![stop(0.0, "#ffffff"), stop(1.0, "#000000")],
            opacity: 1.0,
        });
        let svg = render(node);
        assert!(
            svg.contains("<radialGradient id=\"")
                && svg.contains("cx=\"0.5\" cy=\"0.5\" r=\"0.75\""),
            "got {svg}"
        );
    }

    #[test]
    fn a_tiled_image_says_it_was_not_tiled() {
        let node = Node::new(
            NodeId::from_static("nd_i"),
            NodeKind::Image(ImageGeometry {
                asset: "photo.png".into(),
                width: 100.0,
                height: 80.0,
                fit: ImageFit::Tile,
            }),
        );
        let warns = warnings(node);
        assert!(
            warns.iter().any(|w| w.contains("tile")),
            "tiling was silently swapped for cover: {warns:?}"
        );
    }

    #[test]
    fn an_image_keeps_its_border_but_is_not_painted_over() {
        let mut node = Node::new(
            NodeId::from_static("nd_i"),
            NodeKind::Image(ImageGeometry {
                asset: "photo.png".into(),
                width: 100.0,
                height: 80.0,
                fit: ImageFit::Cover,
            }),
        )
        .with_fill(Paint::solid("#00ff00").unwrap());
        node.strokes
            .push(Stroke::new(Paint::solid("#ff0055").unwrap(), 3.0));

        let svg = render(node);
        assert!(
            svg.contains("<image href=\"assets/photo.png\""),
            "got {svg}"
        );
        assert!(
            svg.contains("stroke=\"#ff0055\""),
            "a bordered photo lost its border: {svg}"
        );
        assert!(
            !svg.contains("fill=\"#00ff00\""),
            "a fill was painted over the photo: {svg}"
        );
    }

    #[test]
    fn an_image_with_no_area_is_reported() {
        let mut node = Node::new(
            NodeId::from_static("nd_i"),
            NodeKind::Image(ImageGeometry {
                asset: "photo.png".into(),
                width: 200.0,
                height: 0.0,
                fit: ImageFit::Cover,
            }),
        );
        node.name = "Hero photo".into();

        let (svg, warns) = write(&page_with(node));
        assert!(
            warns.iter().any(|w| w.contains("Hero photo")),
            "a zero-height image vanished without a word: {warns:?}"
        );
        assert!(
            !svg.contains("<image"),
            "a dead <image> element shipped anyway: {svg}"
        );
    }

    #[test]
    fn a_shape_with_no_area_is_reported_rather_than_dropped() {
        let mut node = rect(120.0, 0.0);
        node.name = "Divider".into();
        let warns = warnings(node.with_fill(Paint::solid("#ff0055").unwrap()));
        assert!(
            warns.iter().any(|w| w.contains("Divider")),
            "a zero-height rect vanished without a word: {warns:?}"
        );
    }

    #[test]
    fn an_ellipse_with_no_area_is_reported_too() {
        let mut node = Node::new(
            NodeId::from_static("nd_e"),
            NodeKind::Ellipse(EllipseGeometry {
                width: 0.0,
                height: 40.0,
            }),
        );
        node.name = "Dot".into();
        let warns = warnings(node.with_fill(Paint::solid("#ff0055").unwrap()));
        assert!(warns.iter().any(|w| w.contains("Dot")), "got {warns:?}");
    }

    #[test]
    fn a_frame_with_no_area_is_reported() {
        let mut node = frame(0.0, 200.0);
        node.name = "Sidebar".into();
        node.fills.push(Paint::solid("#ffffff").unwrap());
        let warns = warnings(node);
        assert!(warns.iter().any(|w| w.contains("Sidebar")), "got {warns:?}");
    }
}
