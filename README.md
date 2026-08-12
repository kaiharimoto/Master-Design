# Master Design

A design tool for graphic, animated websites — built so that an AI coding model and a
person can edit the same document as equals.

Most "AI builds your site" tools make the model a code generator: it emits output, you
edit the output, and the two immediately desynchronize because the model can no longer
read back what you did without re-parsing its own generated code. Master Design inverts
that. There is one document, and both editors change it the same way.

```
                    ┌──────────────────┐
   you ────────────▶│                  │
   (drag, type)     │   the document   │◀──────────── an AI model
                    │                  │              (over MCP)
                    └────────┬─────────┘
                             │
              one patch protocol, one validator,
              one undo stack, one canonical form
                             │
                    ┌────────▼─────────┐
                    │  a static website │
                    └───────────────────┘
```

An edit made by the model is undone with the same Ctrl+Z as a mouse drag. That single
property is what makes it a collaborator rather than something to clean up after.

## Status

Milestone 1: a working slice through every part of the system. The engine — document
model, geometry, animation, export, and the AI bridge — is built, tested and runnable
today. The studio's interface is written and its frontend builds; the desktop and
Android shells build in CI on the platforms they ship to.

**291 tests.** `cargo test --workspace`.

## What is here

| | |
| --- | --- |
| `crates/md-doc` | The document: scene graph, canonical serialization, patch protocol, undo |
| `crates/md-geom` | Vector engine: beziers, booleans, stroke outlining, curve fitting |
| `crates/md-anim` | Animation packages: manifests, registry, declarative baker |
| `crates/md-emit` | Export to a self-contained static site |
| `crates/md-mcp` | The AI bridge: an MCP server with a headless renderer |
| `crates/md-cli` | `md` — everything above, without a GUI |
| `apps/studio` | The editor: Tauri v2 shell, SolidJS interface |
| `packages/md-anim-std` | Six standard animations, as data |

## Try it

```bash
cargo build --release -p md-cli
export MD_ANIMATIONS=packages/md-anim-std/animations
MD=./target/release/md

$MD new /tmp/site --name "Frontier"
$MD describe /tmp/site
```

Add something, animate it, and look at a frame from inside the animation:

```bash
ROOT=$($MD query /tmp/site 'type:frame' | head -1 | awk '{print $2}')

cat > /tmp/ops.json <<EOF
[{"op":"node.insert","parent":"$ROOT","node":{
   "id":"nd_card","type":"rect","roles":["card"],
   "width":300,"height":180,"cornerRadius":[16,16,16,16],
   "transform":[1,0,0,1,100,120],
   "fills":[{"type":"solid","color":"#ff0055"}]}}]
EOF

$MD patch /tmp/site /tmp/ops.json --label "Add a card"
$MD anim apply /tmp/site std/stagger-fade-up '@card'
$MD export /tmp/site --out /tmp/out
$MD snapshot /tmp/site --out /tmp/frame.png --time 0.2
```

That last command renders a PNG a quarter-second into the animation, with no browser and
no display server. It is the same call an AI makes to check its own work.

## Attaching a model

```bash
md mcp /path/to/project
```

Point any MCP client at that. The model gets twelve tools; two of them carry the idea:

- **`doc_snapshot`** renders the page to an image, optionally mid-animation. A node list
  cannot tell you that two elements overlap, that text is illegible against its
  background, or that an entrance starts off-canvas. A picture can.
- **`selection_get`** reads what you have selected, what you typed, and what you drew on
  the canvas. That is how "make this bounce" becomes something answerable.

Everything the model changes goes through `doc_patch`, so it is validated, atomic and
undoable — exactly like your own edits.

There is an asynchronous path too. A phone has no terminal to run an agent in, so a note
written there is queued into the project, travels with it through git, and is picked up
by `requests_list` in a desktop session later.

## Adding an animation

Make a folder with a `manifest.json` in it. That is the whole format.

```json
{
  "id": "std/slide-in-left",
  "version": "1.0.0",
  "title": "Slide In Left",
  "category": "entrance",
  "defaultTrigger": { "type": "view", "threshold": 0.2, "once": true },
  "params": [
    { "key": "distance", "label": "Distance", "type": "number",
      "min": 0, "max": 600, "step": 1, "unit": "px", "default": 80 }
  ],
  "generator": {
    "kind": "declarative",
    "duration": "duration",
    "perTarget": {
      "endAt": "duration",
      "tracks": [
        { "property": "opacity", "from": 0, "to": 1 },
        { "property": "translateX", "from": "=-distance", "to": 0 }
      ]
    }
  }
}
```

It appears in the picker with a working slider. No editor code, no registration, no
rebuild — the parameter list *is* the inspector. See
[`packages/md-anim-std/README.md`](packages/md-anim-std/README.md).

## Requirements, and where they stand

**Windows and Android, touch and mouse, portrait and landscape.** One Tauri v2 codebase
for both. Three shells — desktop, tablet, phone — chosen from the shape of the window and
whether the pointer can hover, not from a platform check, so a Windows tablet in portrait
gets the touch layout. Gestures are normalized once: pinch-zoom, two-finger pan,
long-press, and a loupe that magnifies what your finger is covering while you drag a
handle.

**Native updates from GitHub Releases.** Windows uses a signed manifest and swaps the
binary in place. Android cannot do that — only the system installer can replace an APK —
so it downloads, verifies a SHA-256 published in the release notes, and hands off to the
installer, falling back to the release page if the device refuses.

**Illustrator-grade editing.** Bezier paths, boolean operations with curve refitting,
stroke outlining, live corners, gradients, blend modes and effects. Deep typography —
text on a path, OpenType features, variable-font axes — is deliberately M2; see the
roadmap.

**A growable animation library.** Covered above.

## Building

```bash
cargo test --workspace          # the engine
pnpm install
pnpm --filter @master-design/studio build     # the interface

cd apps/studio && pnpm tauri dev              # the app (needs a system webview)
```

The studio's Rust shell is a separate cargo workspace: the core crates compile anywhere,
but that one needs a system webview or an Android NDK. Keeping it apart means
`cargo test --workspace` runs on a plain machine with no GUI libraries.

Before a first release, generate an update signing key and an Android keystore — see
[`docs/releasing.md`](docs/releasing.md). Without them the app builds and runs fine; it
just cannot update itself in place.

## Reading further

- [`docs/architecture.md`](docs/architecture.md) — how the pieces fit, and why
- [`docs/adr/`](docs/adr/) — the decisions that shaped it, with their trade-offs
- [`docs/spec/document-format.md`](docs/spec/document-format.md) — the on-disk format
- [`docs/spec/patch-protocol.md`](docs/spec/patch-protocol.md) — every operation
- [`docs/roadmap.md`](docs/roadmap.md) — what M1 deliberately left out

MIT.
