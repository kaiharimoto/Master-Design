//! Auto-layout: a frame that places its own children.
//!
//! This is flexbox, deliberately. Not because flexbox is elegant, but because the export
//! target is a browser and a designer's mental model of "a row of cards that wraps" is
//! already shaped by it — inventing a fourth stacking model would mean the tool, the
//! designer and the output all meaning slightly different things by "space between".
//!
//! What is *not* here, and is not an oversight: flex **grow** factors. A child either has
//! its own size, is stretched to the cross axis, or is narrowed to fit. Growing along the
//! main axis needs a notion of flexible children the document model does not have yet, and
//! faking it by distributing slack evenly would produce layouts that look right in one
//! place and wrong everywhere else.
//!
//! Shrinking *is* here, because in CSS it is the default (`flex-shrink: 1`) and because
//! without it the phone breakpoint — the reason any of this exists — puts a row of cards
//! half off the screen.

use crate::{move_to, outer_height, outer_width};
use md_doc::node::{AlignItems, Direction, JustifyContent, Layout, Node, NodeKind};

/// Place `frame`'s children according to `layout`, returning the height the content needs.
///
/// `width` is the frame's own width, already decided by whatever resized it.
pub fn lay_out(frame: &mut Node, layout: &Layout, width: f64) -> f64 {
    let [pad_top, pad_right, pad_bottom, pad_left] = layout.padding;
    let inner_width = (width - pad_left - pad_right).max(0.0);

    // A hidden layer occupies no space, the way `display: none` does — not zero size,
    // *absent*. Anything else leaves a gap where a designer turned something off.
    let visible: Vec<usize> = frame
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.visible)
        .map(|(i, _)| i)
        .collect();

    if visible.is_empty() {
        return pad_top + pad_bottom;
    }

    // Stretch resizes before anything is measured: a stretched text box has to re-wrap at
    // its new width before its height means anything.
    if layout.align == AlignItems::Stretch {
        for &i in &visible {
            stretch_child(&mut frame.children[i], layout.direction, inner_width);
        }
    }

    // Nothing may be wider than the box it is in. CSS gets this from `flex-shrink: 1`,
    // which is the *default* — a 400pt card in a 294pt row shrinks rather than hanging
    // out of it — and from `max-width: 100%` being what anyone means by a column. Without
    // it a phone breakpoint puts the cards half off the screen, which is exactly the
    // failure responsive layout exists to prevent.
    for &i in &visible {
        clamp_to(&mut frame.children[i], inner_width);
    }

    let content_height = match layout.direction {
        Direction::Vertical => column(frame, layout, &visible, inner_width, pad_left, pad_top),
        Direction::Horizontal => row(frame, layout, &visible, inner_width, pad_left, pad_top),
    };

    content_height + pad_top + pad_bottom
}

/// Narrow a child that will not fit, leaving one that does alone.
fn clamp_to(child: &mut Node, available: f64) {
    let Some((w, h)) = child.box_size() else {
        return;
    };
    if w <= available {
        return;
    }
    if matches!(child.kind, NodeKind::Frame(_)) {
        crate::resize_frame(child, available, h, w, h);
    } else {
        child.set_box_size(available, h);
    }
}

/// Give a child the full cross-axis measure.
fn stretch_child(child: &mut Node, direction: Direction, inner_width: f64) {
    // Only the vertical direction has a cross axis this function can serve: stretching
    // across a *row* means matching the tallest sibling, which is decided after
    // measurement rather than before it, and is handled in `row`.
    if direction != Direction::Vertical {
        return;
    }
    let Some((_, h)) = child.box_size() else {
        return;
    };
    if matches!(child.kind, NodeKind::Frame(_)) {
        let (old_w, old_h) = child.box_size().unwrap_or((0.0, 0.0));
        crate::resize_frame(child, inner_width, h, old_w, old_h);
    } else {
        child.set_box_size(inner_width, h);
    }
}

fn column(
    frame: &mut Node,
    layout: &Layout,
    visible: &[usize],
    inner_width: f64,
    pad_left: f64,
    pad_top: f64,
) -> f64 {
    let heights: Vec<f64> = visible
        .iter()
        .map(|&i| outer_height(&frame.children[i]))
        .collect();

    let total: f64 = heights.iter().sum::<f64>() + layout.gap * (visible.len() as f64 - 1.0);

    // `justify` distributes along the main axis, which only has slack when the frame has a
    // stated height larger than its content. A hugging frame has none by definition.
    let stated_height = match &frame.kind {
        NodeKind::Frame(f) => f.height,
        _ => 0.0,
    };
    let slack = (stated_height - total).max(0.0);
    let (mut cursor, extra_gap) = main_axis_start(layout.justify, slack, visible.len(), pad_top);

    for (n, &i) in visible.iter().enumerate() {
        let child_width = outer_width(&frame.children[i]);
        let x = cross_offset(layout.align, pad_left, inner_width, child_width);
        move_to(&mut frame.children[i], x, cursor);
        cursor += heights[n] + layout.gap + extra_gap;
    }

    total.max(0.0)
}

fn row(
    frame: &mut Node,
    layout: &Layout,
    visible: &[usize],
    inner_width: f64,
    pad_left: f64,
    pad_top: f64,
) -> f64 {
    let sizes: Vec<(f64, f64)> = visible
        .iter()
        .map(|&i| {
            (
                outer_width(&frame.children[i]),
                outer_height(&frame.children[i]),
            )
        })
        .collect();

    // Break into lines first, so `justify` can distribute within each one.
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut used = 0.0;

    for (n, &_i) in visible.iter().enumerate() {
        let w = sizes[n].0;
        let with_gap = if current.is_empty() {
            w
        } else {
            used + layout.gap + w
        };

        if layout.wrap && !current.is_empty() && with_gap > inner_width {
            lines.push(std::mem::take(&mut current));
            used = w;
        } else {
            used = with_gap;
        }
        current.push(n);
    }
    if !current.is_empty() {
        lines.push(current);
    }

    let mut y = pad_top;
    for line in &lines {
        let line_width: f64 =
            line.iter().map(|&n| sizes[n].0).sum::<f64>() + layout.gap * (line.len() as f64 - 1.0);
        let line_height = line.iter().map(|&n| sizes[n].1).fold(0.0, f64::max);

        let slack = (inner_width - line_width).max(0.0);
        let (mut cursor, extra_gap) = main_axis_start(layout.justify, slack, line.len(), pad_left);

        for &n in line {
            let i = visible[n];
            let child_height = sizes[n].1;
            let child_y = match layout.align {
                AlignItems::Start => y,
                AlignItems::Center => y + (line_height - child_height) / 2.0,
                AlignItems::End => y + (line_height - child_height),
                // Across a row, stretching means matching the tallest thing on the line,
                // which is only knowable now that the line exists.
                AlignItems::Stretch => {
                    let (w, _) = frame.children[i].box_size().unwrap_or((0.0, 0.0));
                    frame.children[i].set_box_size(w, line_height);
                    y
                }
            };
            move_to(&mut frame.children[i], cursor, child_y);
            cursor += sizes[n].0 + layout.gap + extra_gap;
        }

        y += line_height + layout.gap;
    }

    // The trailing gap belongs between lines, not after the last one.
    (y - pad_top - layout.gap).max(0.0)
}

/// Where the first item starts, and how much extra space goes between items.
fn main_axis_start(justify: JustifyContent, slack: f64, count: usize, pad: f64) -> (f64, f64) {
    match justify {
        JustifyContent::Start => (pad, 0.0),
        JustifyContent::Center => (pad + slack / 2.0, 0.0),
        JustifyContent::End => (pad + slack, 0.0),
        // With one item there is nothing to put space between, and CSS agrees: it sits at
        // the start rather than being centred.
        JustifyContent::SpaceBetween if count > 1 => (pad, slack / (count as f64 - 1.0)),
        JustifyContent::SpaceBetween => (pad, 0.0),
    }
}

fn cross_offset(align: AlignItems, pad: f64, inner: f64, child: f64) -> f64 {
    match align {
        AlignItems::Start | AlignItems::Stretch => pad,
        AlignItems::Center => pad + (inner - child) / 2.0,
        AlignItems::End => pad + (inner - child),
    }
}
