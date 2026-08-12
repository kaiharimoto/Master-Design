//! Emitting one page at several widths.
//!
//! An SVG scales as one picture. That is the right behaviour for a logo and the wrong
//! behaviour for a website: a 1440-wide design shown on a 390-wide phone renders at 27%,
//! and body text set at 16px arrives four pixels tall. No amount of care in the renderer
//! fixes that, because the problem is that the design was made for one width.
//!
//! So the exporter solves the layout at each breakpoint and emits one `<svg>` per band,
//! showing exactly one of them with a media query. The bands come from the project's own
//! breakpoints, which are the same three shapes the studio's interface adapts to.
//!
//! ## What this does and does not buy
//!
//! Within a band the artwork still scales — a phone copy solved at 390 is shown at 1.4×
//! on a 560-wide window. What changes is that the *layout* is right for the band: the
//! cards are in one column, the text wrapped at a phone measure, the header stacked. That
//! is the difference between a page that works on a phone and a page that is technically
//! visible on one.
//!
//! The widest band gets a `max-width`, so the design stops growing rather than filling a
//! 5120-pixel monitor at 3½×.
//!
//! ## Why not one SVG that reflows
//!
//! Because SVG has no layout engine. The reflow happens here, in Rust, against real font
//! metrics, and the result is geometry. Shipping several copies of that geometry costs
//! bytes — they compress extremely well, being near-identical — and buys a page with no
//! runtime layout cost at all: no JavaScript, no reflow, no shift as fonts load.

use md_doc::{Document, Page};

/// One band: a solved page, and the widths it is shown at.
#[derive(Debug, Clone)]
pub struct Band {
    /// The breakpoint's name, used to build stable ids and class names.
    pub name: String,
    /// Width the layout was solved at.
    pub width: f64,
    /// Lower edge of the band, in CSS pixels.
    pub min_width: f64,
    /// Upper edge, exclusive. `None` for the widest band.
    pub max_width: Option<f64>,
    pub page: Page,
}

impl Band {
    /// The media query that selects this band, or `None` for one that is always on.
    pub fn media_query(&self) -> Option<String> {
        match (self.min_width > 0.0, self.max_width) {
            (false, None) => None,
            (true, None) => Some(format!("(min-width:{}px)", round(self.min_width))),
            (false, Some(max)) => Some(format!("(max-width:{}px)", round(max - 1.0))),
            (true, Some(max)) => Some(format!(
                "(min-width:{}px) and (max-width:{}px)",
                round(self.min_width),
                round(max - 1.0)
            )),
        }
    }

    pub fn class(&self) -> String {
        format!("md-bp-{}", sanitize(&self.name))
    }
}

/// The narrowest a band is ever solved at.
///
/// A breakpoint whose lower edge is zero would otherwise be solved at one pixel wide,
/// collapsing every stretched element. 390 is a common small phone, and solving there
/// means the phone copy is at or near 1:1 on the devices that get it.
const NARROWEST: f64 = 390.0;

/// Work out the bands for a page.
///
/// Returns a single band covering everything when the design has nothing responsive in
/// it: three identical copies of a fixed graphic would triple the page for no benefit.
pub fn bands(doc: &Document, page: &Page) -> Vec<Band> {
    if !md_layout::is_responsive(page) || doc.meta.breakpoints.is_empty() {
        return vec![Band {
            name: "all".into(),
            width: page.width,
            min_width: 0.0,
            max_width: None,
            page: page.clone(),
        }];
    }

    // Narrowest first, and deduplicated: two breakpoints at the same width would produce
    // two identical copies and a media query that matches neither.
    let mut points: Vec<(String, f64)> = doc
        .meta
        .breakpoints
        .iter()
        .map(|b| (b.name.clone(), b.min_width.max(0.0)))
        .collect();
    points.sort_by(|a, b| a.1.total_cmp(&b.1));
    points.dedup_by(|a, b| (a.1 - b.1).abs() < 0.5);

    let mut out = Vec::with_capacity(points.len());
    for (i, (name, min_width)) in points.iter().enumerate() {
        let max_width = points.get(i + 1).map(|(_, next)| *next);
        let is_widest = max_width.is_none();

        // Solve at the bottom of the band, so the layout is right for the smallest screen
        // that gets it and scales up from there rather than being cut off. The widest
        // band is the exception: it is solved at the design width, because that is the
        // width the design was actually made at.
        let solve_at = if is_widest {
            page.width.max(*min_width)
        } else {
            min_width.max(NARROWEST)
        };

        out.push(Band {
            name: name.clone(),
            width: solve_at,
            min_width: *min_width,
            max_width,
            page: md_layout::solve(page, solve_at),
        });
    }
    out
}

/// The CSS that shows one band and hides the rest.
///
/// `display:none` rather than visibility: a hidden copy must not be read by a screen
/// reader, must not be tabbed into, and must not lay out. The artwork is already
/// `aria-hidden` — the accessibility outline carries the meaning — but a hidden *copy*
/// would still contribute to the page's size.
pub fn stylesheet(bands: &[Band]) -> String {
    if bands.len() < 2 {
        // One band needs no switching, but it still needs its ceiling: a design should
        // stop growing rather than fill an unusually wide monitor.
        return bands
            .first()
            .map(|b| {
                format!(
                    ".{}{{max-width:{}px;margin:0 auto}}",
                    b.class(),
                    round(b.width)
                )
            })
            .unwrap_or_default();
    }

    let mut css = String::new();
    for band in bands {
        css.push_str(&format!(".{}{{display:none}}", band.class()));
    }
    for band in bands {
        let rule = match band.max_width {
            // The widest band stops growing at the width it was designed at, and centres
            // in whatever is left over.
            None => format!(
                ".{}{{display:block;max-width:{}px;margin:0 auto}}",
                band.class(),
                round(band.width)
            ),
            Some(_) => format!(".{}{{display:block}}", band.class()),
        };
        match band.media_query() {
            Some(query) => css.push_str(&format!("@media {query}{{{rule}}}")),
            None => css.push_str(&rule),
        }
    }
    css
}

fn round(v: f64) -> String {
    md_geom::fmt_coord(v.round())
}

/// Keep a breakpoint name safe to put in a class and an element id.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "bp".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_doc::node::{Constraints, HConstraint, NodeKind, RectGeometry, VConstraint};
    use md_doc::{Breakpoint, Node, NodeId, PageId, Transform};

    fn responsive_page() -> Page {
        let mut p = Page::new(PageId::from_static("pg_1"), "Home", "index", 1440.0, 900.0);
        if let NodeKind::Frame(f) = &mut p.root.kind {
            f.width = 1440.0;
            f.height = 900.0;
        }
        let mut band = Node::new(
            NodeId::from_static("nd_band"),
            NodeKind::Rect(RectGeometry {
                width: 1340.0,
                height: 200.0,
                corner_radius: [0.0; 4],
            }),
        )
        .with_transform(Transform::translate(50.0, 50.0));
        band.constraints = Constraints {
            h: HConstraint::Stretch,
            v: VConstraint::Top,
        };
        p.root.children.push(band);
        p
    }

    fn fixed_page() -> Page {
        let mut p = Page::new(PageId::from_static("pg_1"), "Art", "index", 1440.0, 900.0);
        p.root.children.push(Node::new(
            NodeId::from_static("nd_a"),
            NodeKind::Rect(RectGeometry {
                width: 100.0,
                height: 100.0,
                corner_radius: [0.0; 4],
            }),
        ));
        p
    }

    #[test]
    fn a_responsive_page_gets_one_band_per_breakpoint() {
        let doc = Document::new("Site");
        let bands = bands(&doc, &responsive_page());
        assert_eq!(bands.len(), doc.meta.breakpoints.len());
        assert_eq!(
            bands.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["sm", "md", "lg"]
        );
    }

    #[test]
    fn a_fixed_design_is_emitted_once() {
        let doc = Document::new("Site");
        let bands = bands(&doc, &fixed_page());
        assert_eq!(
            bands.len(),
            1,
            "a design that cannot reflow was copied per breakpoint"
        );
        assert!(bands[0].media_query().is_none());
    }

    #[test]
    fn each_band_is_solved_at_its_own_width() {
        let doc = Document::new("Site");
        let bands = bands(&doc, &responsive_page());
        assert_eq!(bands[0].width, NARROWEST, "the phone band solved too wide");
        assert_eq!(bands[1].width, 768.0);
        assert_eq!(
            bands[2].width, 1440.0,
            "the widest band should use the design width"
        );

        for band in &bands {
            assert_eq!(band.page.width, band.width);
        }
    }

    #[test]
    fn the_stretched_element_actually_reflowed() {
        // Without this the bands would be three copies of the same geometry with
        // different labels on them.
        let doc = Document::new("Site");
        let bands = bands(&doc, &responsive_page());
        let width_of = |b: &Band| {
            b.page
                .root
                .find(&NodeId::from_static("nd_band"))
                .and_then(|n| n.box_size())
                .unwrap()
                .0
        };
        assert_eq!(width_of(&bands[0]), 290.0); // 390 − 50 − 50
        assert_eq!(width_of(&bands[1]), 668.0);
        assert_eq!(width_of(&bands[2]), 1340.0);
    }

    #[test]
    fn media_queries_tile_the_whole_range_without_overlapping() {
        let doc = Document::new("Site");
        let bands = bands(&doc, &responsive_page());

        assert_eq!(bands[0].media_query().as_deref(), Some("(max-width:767px)"));
        assert_eq!(
            bands[1].media_query().as_deref(),
            Some("(min-width:768px) and (max-width:1279px)")
        );
        assert_eq!(
            bands[2].media_query().as_deref(),
            Some("(min-width:1280px)")
        );
    }

    #[test]
    fn every_band_is_hidden_before_one_is_shown() {
        // Order matters: if the reveal rules came first the blanket hide would undo them.
        let doc = Document::new("Site");
        let css = stylesheet(&bands(&doc, &responsive_page()));

        let first_show = css.find("display:block").unwrap();
        let last_hide = css.rfind("display:none").unwrap();
        assert!(
            last_hide < first_show,
            "a hide rule came after a show rule and would win: {css}"
        );
    }

    #[test]
    fn the_widest_band_stops_growing() {
        let doc = Document::new("Site");
        let css = stylesheet(&bands(&doc, &responsive_page()));
        assert!(
            css.contains("max-width:1440px"),
            "a design with no ceiling fills a 5K monitor at 3½×: {css}"
        );
    }

    #[test]
    fn a_single_band_still_gets_a_ceiling() {
        let doc = Document::new("Site");
        let css = stylesheet(&bands(&doc, &fixed_page()));
        assert!(css.contains("max-width:1440px"), "got {css}");
        assert!(!css.contains("display:none"));
    }

    #[test]
    fn duplicate_breakpoints_do_not_produce_a_dead_copy() {
        let mut doc = Document::new("Site");
        doc.meta.breakpoints = vec![
            Breakpoint {
                name: "a".into(),
                min_width: 0.0,
            },
            Breakpoint {
                name: "b".into(),
                min_width: 0.0,
            },
            Breakpoint {
                name: "c".into(),
                min_width: 900.0,
            },
        ];
        let bands = bands(&doc, &responsive_page());
        assert_eq!(bands.len(), 2, "an unreachable band was emitted");
    }

    #[test]
    fn breakpoints_out_of_order_are_sorted_rather_than_trusted() {
        let mut doc = Document::new("Site");
        doc.meta.breakpoints = vec![
            Breakpoint {
                name: "wide".into(),
                min_width: 1000.0,
            },
            Breakpoint {
                name: "narrow".into(),
                min_width: 0.0,
            },
        ];
        let bands = bands(&doc, &responsive_page());
        assert_eq!(bands[0].name, "narrow");
        assert_eq!(bands[1].name, "wide");
        assert_eq!(bands[0].max_width, Some(1000.0));
    }

    #[test]
    fn a_name_that_is_not_a_class_is_made_into_one() {
        assert_eq!(sanitize("sm"), "sm");
        assert_eq!(sanitize("Extra Wide!"), "extra-wide");
        assert_eq!(sanitize("  "), "bp");
        assert_eq!(sanitize("--"), "bp");
    }

    #[test]
    fn a_page_with_no_breakpoints_configured_is_emitted_once() {
        let mut doc = Document::new("Site");
        doc.meta.breakpoints.clear();
        assert_eq!(bands(&doc, &responsive_page()).len(), 1);
    }
}
