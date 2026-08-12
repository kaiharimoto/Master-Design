# fixtures

Documents that exist to be rendered, not to be edited.

## `parity/`

The document `renderer-parity.mjs` compares the two renderers against. It is deliberately
not a pretty design — every element in it is there because it exercises something the two
renderers implement separately and could implement differently:

| Element | What it pins down |
| --- | --- |
| A heading with a measure and negative tracking | Shaping, kerning, and greedy line breaking |
| A paragraph at 1.6 line height | Half-leading — where a baseline sits inside a taller line box |
| An uppercased, centred caption | `text-transform` baked into the glyphs rather than left to CSS, and `text-anchor` |
| Italic text with a stroke | Face selection by style, and `paint-order` on outlined text |
| A gradient rectangle with a drop shadow | Gradient stop interpolation, and filter geometry |
| A rotated ellipse with a radial gradient and a stroke | Transformed gradient space, stroke alignment |
| An even-odd path at 80% opacity | Fill rule, and group opacity vs paint opacity |
| A capsule frame clipping an oversized child | Clip paths, and corner radii larger than the box |

If you add a feature to `md-emit`, add something here that uses it. A feature only one
renderer implements passes every unit test in the repository and fails a user.

To see what the check sees:

```sh
cargo run -p md-cli -- export fixtures/parity --out /tmp/parity-dist
cargo run -p md-cli -- snapshot fixtures/parity --out /tmp/parity.png --width 900
node .github/scripts/renderer-parity.mjs /tmp/parity-dist/index.html /tmp/parity.png 900 700
```

Both images are blurred before comparison, because resvg and Skia anti-alias differently
and comparing raw pixels measures the anti-aliasing rather than the design. A one-point
change to the heading's font size — under 2% — moves the score from 0.36% to 3.27%, which
is the sensitivity the check is tuned for.
