# 2. Animation packages are data, not code

Accepted. Supersedes the original plan, which specified an `index.js` per package.

## Context

The library has to keep growing without the app growing with it: adding an animation
should be adding a folder. The obvious design is a folder with a JavaScript generator in
it — maximally expressive, and what every comparable system does.

## Decision

A package is a folder with a `manifest.json`. The manifest declares its parameters and,
declaratively, how those become keyframes. Values may be numbers, parameter references
(`"$tint"`), or arithmetic expressions (`"=stagger * index"`), evaluated by a small
expression evaluator with per-target variables including `index`, `count`, `width`,
`height` and `pathLength`.

A `script` generator is reserved for genuinely procedural motion. Nothing in the standard
library needs it.

## Consequences

Good:

- Baking needs no JavaScript runtime, so `md-cli`, the exporter and the MCP server can all
  apply an animation. An AI working over MCP with no studio open can animate a page —
  which, given the product is an AI bridge, is not a nice-to-have.
- The `params` array *is* the inspector: a ranged number becomes a slider, a colour a
  swatch, an easing a curve picker. A new animation arrives with working controls and no
  editor code was written for it.
- A project can ship animations in `animations/` without that being a request to execute
  arbitrary code when the document opens.
- Manifests are reviewable and diffable.

Costs:

- Motion that cannot be expressed as arithmetic over parameters cannot be a standard
  package. Physics, path-following, and anything sampling geometry it cannot reach are out
  until the `script` generator is implemented.
- The expression language is one more thing to learn, and it is not a real language — no
  conditionals, no loops. `clamp`, `lerp` and `min`/`max` cover most of what conditionals
  would have been used for.

## Alternatives considered

**JavaScript generators everywhere.** More expressive, and would have meant embedding a JS
engine in the exporter and the CLI, or accepting that headless baking is impossible. The
second was disqualifying.

**Both, with JS as a fallback from day one.** Two code paths and two sets of bugs before
there is a single animation that needs the second one.
