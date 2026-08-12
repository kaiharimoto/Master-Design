# 3. Render to SVG, and ship semantics separately

Accepted.

## Context

The output is a *graphic* website: arbitrary vector artwork, not a stack of rectangles. It
has to look in a browser exactly like it looked on the canvas.

## Decision

The canvas and the exporter both emit SVG. Live shapes (rectangles, ellipses) stay live in
the document and bake to path data only when something needs an outline.

Because SVG carries no semantics, the export additionally ships a **parallel hidden
document** — real headings, paragraphs, landmarks and alt text, visually hidden but in the
accessibility tree — while the artwork itself is marked `aria-hidden`.

## Consequences

Good:

- Fidelity is structural rather than approximated: the canvas is the output in a live DOM,
  not a reconstruction in a second layout model.
- Text stays real `<text>`, so it is selectable and scales cleanly.
- Live shapes keep their parameters, so a corner radius is still editable afterwards.
- Screen readers and search engines get a real document rather than a picture.

Costs:

- A design scales as a whole rather than reflowing. Breakpoint-aware layout is M2; the
  `layout` and `constraints` fields exist in the document model for it.
- Two renderers exist — reactive DOM and string emitter — and they must agree. The shared
  vocabulary of node kinds keeps them honest, and divergence shows up immediately as a
  design that looks different once exported.
- The hidden outline is a second thing that can go stale relative to the artwork. Deriving
  it from the same tree, rather than authoring it separately, is what stops that.

## Alternatives considered

**Positioned HTML boxes.** Better for text-heavy reflowing pages; hopeless for arbitrary
vector artwork, which is the point of the tool.

**Canvas or WebGL.** Fastest for very large scenes, and gives up text selection,
accessibility, and the ability to export anything a browser can render without a runtime.
The renderer sits behind an interface so a GPU backend can be added for the cases that
need it.

**ARIA inside the SVG.** Support is patchy and the reading order follows paint order
rather than meaning.
