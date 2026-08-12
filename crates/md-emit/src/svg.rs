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
    pub fn page(&mut self, page: &Page) -> String {
        let body = self.node(&page.root);

        let background = page
            .background
            .as_ref()
            .map(|p| {
                let paint = self.paint_value(p, "page");
                format!(
                    "<rect width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                    num(page.width),
                    num(page.height),
                    paint
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
                        // A frame's own fill is a rectangle behind its children.
                        let mut out = String::new();
                        if !node.fills.is_empty() {
                            let shape =
                                md_geom::rect_path(0.0, 0.0, f.width, f.height, f.corner_radius);
                            out.push_str(&self.shape_with_paints(node, &shape, ""));
                        }
                        if f.clip {
                            let clip_id = self.next_id("clip");
                            let shape =
                                md_geom::rect_path(0.0, 0.0, f.width, f.height, f.corner_radius);
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
                let href = format!("assets/{}", i.asset);
                format!(
                    "<image href=\"{}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"{}\"/>",
                    esc_attr(&href),
                    num(i.width),
                    num(i.height),
                    match i.fit {
                        md_doc::paint::ImageFit::Cover => "xMidYMid slice",
                        md_doc::paint::ImageFit::Contain => "xMidYMid meet",
                        md_doc::paint::ImageFit::Fill => "none",
                        md_doc::paint::ImageFit::Tile => "xMidYMid slice",
                    }
                )
            }
            _ => {
                let shape = node.geometry_path().unwrap_or_default();
                let extra = match &node.kind {
                    NodeKind::Path(p) => {
                        if p.fill_rule == md_geom::FillRule::EvenOdd {
                            " fill-rule=\"evenodd\""
                        } else {
                            ""
                        }
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
            let opacity = fill.opacity();
            let op_attr = if opacity < 1.0 {
                format!(" fill-opacity=\"{}\"", num(opacity))
            } else {
                String::new()
            };
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
                    return format!("<path d=\"{}\" fill=\"{paint}\"/>", esc_attr(&outlined));
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
            " stroke=\"{paint}\" stroke-width=\"{}\" fill=\"none\"",
            num(stroke.width)
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

    fn text(&mut self, node: &Node, t: &md_doc::TextGeometry) -> String {
        let f = &t.font;
        let anchor = t.align.text_anchor();

        let x = match t.align {
            md_doc::TextAlign::Center => t.width.unwrap_or(0.0) / 2.0,
            md_doc::TextAlign::Right => t.width.unwrap_or(0.0),
            _ => 0.0,
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
        if let Some(case) = f.text_case.as_css() {
            let _ = write!(attrs, " style=\"text-transform:{case}\"");
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

        let fill = match node.fills.first() {
            Some(p) => self.paint_value(p, &format!("{}-t", node.id)),
            // Unfilled text would be invisible, which is never what was meant.
            None => Color::default().to_string(),
        };
        let _ = write!(attrs, " fill=\"{fill}\"");

        let line_height = t.line_height_units();
        // Approximate ascent. Real metrics need the font, which the exporter does not
        // load; the editor measures properly and this is close enough that a design does
        // not shift visibly between the two.
        let first_baseline = f.font_size * 0.8;

        let lines: Vec<String> = t
            .hard_lines()
            .iter()
            .enumerate()
            .map(|(i, line)| {
                format!(
                    "<tspan x=\"{}\" y=\"{}\">{}</tspan>",
                    num(x),
                    num(first_baseline + line_height * i as f64),
                    esc_text(line)
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

    fn filter(&mut self, effects: &[Effect]) -> String {
        let id = self.next_id("filter");
        let mut body = String::new();
        for effect in effects {
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
                    // Composited from the inverse of the source alpha; SVG has no
                    // primitive for this, so it is built out of the ones it does have.
                    let _ = write!(
                        body,
                        "<feOffset dx=\"{}\" dy=\"{}\"/><feGaussianBlur stdDeviation=\"{}\"/>\
                         <feComposite operator=\"out\" in2=\"SourceAlpha\"/>\
                         <feFlood flood-color=\"{}\" flood-opacity=\"{}\"/>\
                         <feComposite operator=\"in\" in2=\"SourceAlpha\"/>\
                         <feComposite operator=\"over\" in2=\"SourceGraphic\"/>",
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
