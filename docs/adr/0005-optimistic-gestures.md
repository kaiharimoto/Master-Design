# 5. Gestures are optimistic; commits are single

Accepted.

## Context

The backend owns the document (ADR 1), so every change is an inter-process round trip. A
drag produces pointer events at display refresh rate. Sending an operation per event would
put a round trip in every frame and two hundred entries on the undo stack for one gesture.

## Decision

A gesture writes to a transient `live` layer in the frontend store, which the renderer
composes on top of the document. Nothing is sent until the pointer lifts, at which point
one operation per affected node is committed as a single undo step.

The live transform is applied via a separate wrapper element rather than by modifying the
node's own `transform`, so the document's matrix is never rewritten mid-gesture.

## Consequences

Good:

- The canvas runs at pointer speed regardless of round-trip latency — which matters most on
  the phone, where latency is worst.
- One drag is one undo step, which is how a person remembers working.
- A cancelled gesture is a discarded local value; nothing to roll back.

Costs:

- Live state is a second place transform information lives, and the two must be composed
  correctly wherever bounds are computed. `withLive` is the single point where that
  happens.
- Validation is deferred to commit. A drag can produce something the backend rejects, and
  the rejection arrives after the gesture has visibly finished. Transforms are always
  valid in practice; the risk is real for future tools editing constrained values.
- Anything watching the document — an attached AI — sees nothing during a drag. That is
  arguably correct: intermediate frames of a gesture are not intent.

## A related consequence in the exporter

The same wrapper trick appears at export time for a different reason: SVG's `transform`
attribute and the CSS `transform` property are the same property, so an animation setting
`transform` on a node would overwrite its placement matrix and move it to the origin.
Animated nodes get their own element to move in.
