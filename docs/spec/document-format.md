# The document format

A project is a **directory**, not a file.

```
my-site/
  project.json      metadata, breakpoints, the page index
  tokens.json       colour, type and spacing scales   (omitted when empty)
  pages/*.json      one scene graph per page
  assets/           images and fonts
  animations/       project-local animation packages
  requests/         queued notes from the human to the AI
  .md/              session state — the current selection; git-ignored
```

Splitting it up is what makes the format work for two editors at once. An AI editing one
page rewrites one file, so a diff shows the page that changed rather than one enormous
blob. Assets stay out of the text entirely.

## Canonical form

There is exactly one byte sequence per document state:

- **keys sorted** — a `BTreeMap`, so it survives struct field reordering
- **floats quantized** to six decimal places, killing accumulated drift
- **whole numbers without an empty fraction** — `800`, not `800.0`
- **arrays of primitives inlined** — a transform is six numbers on one line
- **trailing newline**

Without this the two editors would produce competing spellings of the same state and every
handoff would produce a diff full of noise — and worse, each save would revert the other's
formatting. Path geometry is exempt: it lives inside `d` strings already formatted by the
geometry kernel at four decimal places.

## Nodes

Kind-specific fields are flattened alongside the common ones, tagged by `type`.

```json
{
  "id": "nd_01hq5...",
  "type": "rect",
  "name": "Card",
  "roles": ["card"],
  "transform": [1, 0, 0, 1, 100, 320],
  "width": 300,
  "height": 180,
  "cornerRadius": [16, 16, 16, 16],
  "fills": [{ "type": "solid", "color": "#ff0055" }],
  "a11y": { "headingLevel": 1 }
}
```

Kinds: `frame`, `group`, `path`, `rect`, `ellipse`, `text`, `image`. Only frames and groups
may hold children; validation rejects children anywhere else.

Defaults are omitted on write — `visible`, `opacity`, an identity transform and an empty
`children` array never reach the file.

### roles

Semantic tags, lowercase kebab-case by convention. Animations and AI instructions address
*roles*, not ids, which is what lets one animation package apply to any document and lets
"stagger the cards" resolve without anyone knowing an id.

### transform

`[a, b, c, d, e, f]` — the same six numbers as SVG's `matrix()`, so a node's transform goes
straight into a rendered attribute with no conversion.

### Paths

Stored as SVG path data strings, normalized to absolute cubics (`M`/`C`/`Z`). The format is
already understood by the browser, by every design tool, and by the models that edit these
documents; it diffs as one line, and export is a no-op.

Rectangles and ellipses stay **live** — parameters rather than baked geometry — so a corner
radius is still editable afterwards. They bake to paths only when something needs an
outline.

### Colours

`#rrggbb` or `#rrggbbaa`, lowercase. Shorthand expands on parse and a redundant `ff` alpha
is dropped, so one colour has exactly one spelling.

## Timelines

A timeline stores two descriptions of the same motion:

```json
{
  "id": "tl_01hq5...",
  "name": "Cards in",
  "trigger": { "type": "view", "threshold": 0.2, "once": true },
  "duration": 0.84,
  "source": {
    "package": "std/stagger-fade-up",
    "version": "1.0.0",
    "target": "@card",
    "params": { "distance": 48, "stagger": 0.12 }
  },
  "tracks": []
}
```

`source` is the editable **intent**; `tracks` are the baked **result**. Keeping both looks
redundant until you notice that generators run in Rust while the studio is a webview, and
that a document opened without its animation packages still renders correctly — it just
cannot re-parameterize until the package is back. Changing a parameter re-bakes; nothing
else regenerates tracks.

Keyframe times are normalized 0..1. Scroll-linked timelines have no clock, so their
progress comes from scroll position instead.

## Requests

A note from the human to the AI, waiting to be picked up.

```json
{
  "id": "req_1737000000000",
  "createdAt": 1737000000000,
  "note": "the cards feel too static",
  "page": "index",
  "selection": ["nd_card0"],
  "status": "open"
}
```

This is the asynchronous half of the bridge. There is no terminal on a phone to run an
agent in, so a note written there is committed into the project and picked up by a session
on a desktop later.

## Versioning

`schemaVersion` is refused when newer than the reader understands, with a message saying to
update rather than a parse error.
