# Standard animation library

Six animations, each a folder with a `manifest.json` in it. That is the whole format.

| Package | Category | What it does |
| --- | --- | --- |
| `std/stagger-fade-up` | entrance | Elements rise into place one after another |
| `std/pop-in` | entrance | Scales up with an overshoot |
| `std/draw-path` | entrance | A stroked path draws itself on |
| `std/scroll-parallax` | scroll | Drifts with scroll position, for depth |
| `std/float` | ambient | A slow endless bob |
| `std/hover-lift` | interaction | Raises under the pointer |

## Adding one

Create a folder, write a `manifest.json`, restart. It appears in the picker with working
controls. There is no registration step, no rebuild, and no editor code to write — the
`params` array *is* the inspector.

```json
{
  "id": "std/slide-in-left",
  "version": "1.0.0",
  "title": "Slide In Left",
  "category": "entrance",
  "defaultTrigger": { "type": "view", "threshold": 0.2, "once": true },
  "params": [
    { "key": "distance", "label": "Distance", "type": "number",
      "min": 0, "max": 600, "step": 1, "unit": "px", "default": 80 },
    { "key": "duration", "label": "Duration", "type": "number",
      "min": 0.05, "max": 5, "step": 0.05, "unit": "s", "default": 0.5 }
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

Each parameter type maps to a control: `number` with a range becomes a slider, `color` a
swatch, `easing` a curve picker, `select` a dropdown, `boolean` a switch.

## Values in a generator

| Written as | Means |
| --- | --- |
| `32` | the number 32 |
| `"=distance * 2"` | arithmetic over parameters and node variables |
| `"$tint"` | the value of the `tint` parameter, whatever its type |
| `"#ff0055"` | a literal string |

Expressions can use `+ - * / %`, parentheses, and `min max abs floor ceil round sqrt
clamp lerp pow`.

Available variables: every numeric or boolean parameter, plus `index` (position in the
selection), `count` (how many targets), `width`, `height`, `x`, `y`, `pathLength` and
`siblingIndex` for the current target. `pathLength` is why `draw-path` can make lines of
different lengths draw at a matched *speed* rather than a matched duration.

## Why there is no JavaScript here

Manifests are data so that `md-cli`, the exporter and the MCP server can bake animations
without a JavaScript runtime. An AI agent working over MCP with no studio open can still
animate a page. It also means a project can ship its own animations in `animations/`
without that being a request to execute arbitrary code at document-open time.

Genuinely procedural motion is what `"kind": "script"` is reserved for. Nothing here
needs it, and reaching for it costs headless baking, so it is worth trying hard to stay
declarative.

## Previews

Packages do not ship preview images. The picker generates them by baking the animation
against a stock three-square scene and playing it, so a preview is always what the
animation currently does rather than what it did when someone last exported a thumbnail.
A package may still include a `preview.svg` to override that.

## Versioning

Documents pin the exact version they were baked against. Installing a newer package
never silently changes existing work — upgrading is a deliberate re-bake. Bump the minor
version for new parameters with defaults, and the major version for anything that would
change how existing documents look.
