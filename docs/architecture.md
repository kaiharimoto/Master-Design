# Architecture

## The bet

Master Design has two editors of equal standing: a person, and an AI model. Everything
below follows from refusing to privilege either one.

The usual arrangement — model generates code, human edits code — fails at the first
handoff. Once a person touches generated output, the model can only understand what
changed by re-reading and re-inferring its own generated code, and any structure it had
in mind is gone. Round-tripping gets worse with every pass.

So the shared artifact is the **document**, not the code, and neither editor writes to it
directly. Both go through the same operations:

```
  mouse drag ─┐
              ├─▶ Op[] ─▶ validate ─▶ apply to a copy ─▶ swap ─▶ inverse ops ─▶ undo stack
  AI patch ───┘
```

Four things fall out of that, and each is load-bearing:

1. **One history.** An AI edit is undone with the same keystroke as a drag. If it were
   not, nobody would let a model touch anything.
2. **One validator.** A model cannot produce a node the GUI could not have produced.
3. **Atomicity.** Operations apply to a copy which is swapped on success, so a patch that
   fails half-way leaves the document untouched rather than half-edited.
4. **One canonical form.** Exactly one byte sequence per document state, so the two
   editors cannot fight over formatting and git diffs stay readable.

## The parts

```
                    ┌──────────────┐
                    │   md-geom    │  beziers, booleans, stroke outlining, fitting
                    └──────┬───────┘  (also compiles to WebAssembly)
                           │
                    ┌──────▼───────┐
                    │   md-doc     │  the document, the patch protocol, undo,
                    └──────┬───────┘  selectors, canonical serialization
             ┌─────────────┼─────────────┐
      ┌──────▼─────┐ ┌─────▼──────┐ ┌────▼─────┐
      │  md-anim   │ │  md-emit   │ │  md-mcp  │
      │  packages  │ │  export    │ │  the AI  │
      │  + baking  │ │  + frames  │ │  bridge  │
      └──────┬─────┘ └─────┬──────┘ └────┬─────┘
             └─────────────┼─────────────┘
                  ┌────────┴────────┐
            ┌─────▼─────┐    ┌──────▼──────┐
            │  md-cli   │    │ apps/studio │
            │ headless  │    │ the editor  │
            └───────────┘    └─────────────┘
```

Nothing above `md-doc` knows about the studio, and nothing in the core crates knows about
Tauri. That is what lets `md-cli`, the MCP server and the app all be thin.

## Why the exporter is Rust and animations are data

These two decisions are the same decision.

An animation package could have been a folder with an `index.js` in it — more expressive,
and the obvious design. But then baking an animation would need a JavaScript runtime,
and the exporter, the CLI and the MCP server would each need one too. An AI working over
MCP with no studio open could not animate a page at all.

So manifests are declarative: parameters, and arithmetic over them. A small expression
evaluator covers what motion actually needs (`duration + stagger * (count - 1)`,
`pathLength`), and Rust bakes it. Three consequences worth the constraint:

- Headless baking works everywhere.
- Adding an animation is adding a folder — the manifest's `params` array *is* the
  inspector, so a new animation arrives with working controls and no editor code.
- A project can ship its own animations in `animations/` without that being a request to
  execute arbitrary code at document-open time.

Genuinely procedural motion is left to a `script` generator that nothing yet needs.

## Where the document lives at runtime

In the backend, not the webview. The frontend holds a reactive mirror and asks for
changes.

The cost is a round trip per change, which would be unacceptable during a drag. So drags
are **optimistic**: a gesture writes to a transient `live` layer the renderer composes on
top of the document, and commits exactly one operation when the pointer lifts. The canvas
runs at pointer speed regardless of latency, and one drag is one undo step — which is
also how a person remembers working.

## Two renderers, one vocabulary

There are unavoidably two: the studio's is a reactive DOM tree, the exporter's builds a
string. They must agree, and the shared vocabulary of `md-doc` node kinds is what keeps
them honest. Divergence shows up immediately as a design that looks different once
exported, which is the failure mode that gets noticed rather than the one that festers.

The canvas renders the same SVG primitives the exporter writes, so what you see is the
output in a live DOM rather than an approximation of it.

## What SVG costs, and what pays for it

SVG carries no semantics. A heading drawn as vector text is, to a screen reader or a
search engine, a shape.

Rather than litter the SVG with ARIA — patchy support, and a reading order that follows
paint order rather than meaning — the export ships a **parallel hidden document** with
real `<h1>`s, paragraphs, landmarks and alt text, while the artwork is marked
`aria-hidden`. The accessibility metadata is entered in the inspector, because it cannot
be retrofitted onto flat vector output afterwards.

## The bridge, concretely

Synchronous, when an agent is attached: the studio writes `.md/selection.json` with the
selected nodes, the note typed, the viewport, and any annotation drawn. `selection_get`
reads it. "Make this bounce" becomes answerable because "this" has ids attached.

Asynchronous, when one is not: the note is queued into `requests/` in the project, which
travels through git and is picked up by `requests_list` later. This is the only path that
exists on a phone, and designing around it is what keeps the mobile app from being a
viewer.

`doc_snapshot` closes the loop in the other direction — the model renders what it built,
optionally mid-animation, and looks at it.

## Testing

The properties that matter are tested as properties, not as examples:

- every operation's inverse restores the document **byte for byte**
- canonical serialization is a fixed point — serialize, parse, serialize is stable
- a failed patch leaves the document and the history untouched
- an AI patch and a GUI edit interleave on one undo stack
- every shipped animation manifest bakes against a real scene
- the exported page is loaded in a real browser and checked, including that motion is
  suppressed under `prefers-reduced-motion`

`crates/md-mcp/tests/bridge.rs` drives a full MCP conversation end to end. If it breaks,
the AI half of the product is broken whatever else still passes.
