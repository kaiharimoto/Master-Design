# Roadmap

Milestone 1 proved the architecture end to end rather than finishing any one part of it.
This is what it deliberately left, and why each thing was left rather than rushed.

## Typography (M2)

Shipped: font family, size, weight, style, tracking, leading, alignment, case,
decoration, and character-range overrides.

Not yet: text on a path, OpenType feature toggles, variable-font axes, optical kerning,
proper line breaking with real font metrics, and text wrapping inside a shape.

Left out because it is deep enough to have swallowed the whole milestone on its own, and
because the shape of `TextSpan` is what those hang off — adding them is additive rather
than a rewrite. Until then the exporter estimates text bounds; the studio measures for
real via `getBBox`.

## Responsive layout (M2)

The document model already carries `layout` and `constraints`, and the project carries
breakpoints. Neither is applied yet: an exported page scales as a whole rather than
reflowing.

This is the largest gap between "graphic website" and "website". It was left because
getting constraint solving wrong produces layouts that are subtly off in ways nobody can
debug, and because scaling is genuinely correct for a large class of graphic pages.

## Vector depth (M2)

Shipped: bezier editing, booleans with curve refitting, stroke outlining, live corners,
gradients, blend modes, blur and shadows.

Not yet: gradient mesh, variable-width strokes, pattern fills, a real pencil tool with
pressure, path offsetting, and the pen tool's full behaviour — currently click-to-place
corners rather than click-and-drag for handles.

Boolean operations flatten, clip and refit, which is what every shipping vector tool does;
the output is accurate to the flattening tolerance rather than exact. Exact bezier-bezier
clipping is a research project, not a milestone.

## Animation (M2)

Shipped: six standard packages, declarative baking, six trigger kinds, per-timeline
reduced-motion behaviour, scrubbing.

Not yet: the `script` generator for procedural motion, motion along a path, spring
physics, per-keyframe editing in the timeline, and animation of properties beyond the
twelve the exporter compiles.

## The studio (ongoing)

Written and building; the parts that need real device testing are the parts that need
real devices. Specifically untested on hardware: the loupe under a finger, pinch-zoom
precision, rotation mid-edit, and how the phone shell holds up on a small screen.

Not yet: snapping and smart guides, a proper alignment panel, component instances, and
on-canvas text editing (text is edited in the inspector).

## Updates

Windows and Android are implemented. Both need a signing key before a first release —
see `docs/releasing.md`. Not yet: release channels (stable/beta), and delta updates.

## Known limitations worth stating plainly

- **Two renderers exist** — the studio's reactive DOM and the exporter's string builder —
  and they can drift. They share a vocabulary rather than an implementation.
- **Fonts are not bundled.** A document naming a font the machine does not have falls
  back, and a headless snapshot on a bare machine will render text in whatever it finds.
- **The studio's Rust shell has no automated tests.** It is thin translation over crates
  that are heavily tested, but thin is not zero.
- **Concurrent editing is last-write-wins.** One person and one agent coordinating
  through a file is the supported model; two agents editing at once is not.
