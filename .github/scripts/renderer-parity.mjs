/**
 * Do the two renderers actually agree?
 *
 * There are two of them and there always will be. `crates/md-emit` writes SVG that a
 * browser paints, and `crates/md-mcp` rasterizes with resvg so an AI model can look at a
 * page without a display server. Every feature has to be implemented in both, and the
 * failure mode is not a crash — it is a screenshot that quietly does not match what the
 * visitor sees, which is worse, because the model then "fixes" a problem that is not
 * there.
 *
 * Being careful is not a mechanism. This is: render the same document both ways at the
 * same size and compare the pixels.
 *
 * Usage: node renderer-parity.mjs <exported-index.html> <resvg-snapshot.png> <width> <height>
 */

import { chromium } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { readFile } from "node:fs/promises";
import { PNG } from "pngjs";

const [pageArg, snapshotArg, widthArg, heightArg] = process.argv.slice(2);
if (!pageArg || !snapshotArg) {
  console.error("usage: renderer-parity.mjs <index.html> <snapshot.png> <width> <height>");
  process.exit(2);
}

const width = Number(widthArg ?? 900);
const height = Number(heightArg ?? 700);

/**
 * How much disagreement is acceptable, and how to stop measuring the wrong thing.
 *
 * resvg and Skia rasterize the same geometry with different anti-aliasing and different
 * hinting, so *every* glyph edge and every curve differs by a shade across a pixel or
 * two. Compared raw, a page of body copy disagrees on nearly 2% of its pixels while
 * being, to a reader, identical — which leaves no room to detect the differences that
 * matter: a font that did not load, a line that wrapped at a different measure, an effect
 * one renderer implements and the other does not.
 *
 * So both images are box-blurred before comparison. A blur is exactly the filter that
 * removes a one-pixel edge treatment while leaving a displaced or missing glyph fully
 * present, and it turns a 1.8% "pass" that measured nothing into a fraction of a percent
 * with real headroom underneath it.
 */
const BLUR_RADIUS = 2;
const CHANNEL_TOLERANCE = 24;
const MAX_DIFFERENT_FRACTION = 0.004;

// `MD_CHROMIUM` lets a machine that already has a browser point at it rather than
// downloading a second copy — which is the situation in the development container, where
// the pinned revision and the installed one do not match.
const browser = await chromium.launch(
  process.env.MD_CHROMIUM ? { executablePath: process.env.MD_CHROMIUM } : {},
);
let failed = false;

try {
  const page = await browser.newPage({
    viewport: { width, height },
    deviceScaleFactor: 1,
  });

  const url = pathToFileURL(resolve(pageArg)).href;
  await page.goto(url, { waitUntil: "load" });

  // Fonts are fetched asynchronously; screenshotting before they arrive compares a
  // fallback face against the real one and fails for the wrong reason.
  await page.evaluate(() => document.fonts.ready);

  // The artwork only — not the body around it, whose background is the page colour and
  // whose size is the viewport rather than the page.
  const canvas = await page.locator("svg.md-canvas").first();
  const shot = await canvas.screenshot({ type: "png" });

  const browserImage = PNG.sync.read(shot);
  const resvgImage = PNG.sync.read(await readFile(resolve(snapshotArg)));

  console.log(
    `browser ${browserImage.width}×${browserImage.height}, ` +
      `resvg ${resvgImage.width}×${resvgImage.height}`,
  );

  if (browserImage.width !== resvgImage.width || browserImage.height !== resvgImage.height) {
    console.error("FAIL the two renderers produced different dimensions");
    failed = true;
  } else {
    const { width: w, height: h } = browserImage;
    const total = w * h;

    const a = blur(flatten(browserImage), w, h, BLUR_RADIUS);
    const b = blur(flatten(resvgImage), w, h, BLUR_RADIUS);

    let different = 0;
    for (let i = 0; i < total; i++) {
      if (Math.abs(a[i] - b[i]) > CHANNEL_TOLERANCE) different++;
    }

    const fraction = different / total;
    const verdict = fraction <= MAX_DIFFERENT_FRACTION ? "ok  " : "FAIL";
    console.log(
      `${verdict} ${different} of ${total} pixels differ ` +
        `(${(fraction * 100).toFixed(2)}%, limit ${(MAX_DIFFERENT_FRACTION * 100).toFixed(2)}%)`,
    );
    if (fraction > MAX_DIFFERENT_FRACTION) failed = true;
  }
} finally {
  await browser.close();
}

/**
 * One luminance value per pixel, composited over white.
 *
 * Over white because a transparent pixel and a white one look the same to a visitor, and
 * the two renderers disagree about which they produce outside the artwork. Luminance
 * because a hue shift of a few units is not a rendering divergence worth failing over,
 * while anything that moves ink moves brightness.
 */
function flatten(image) {
  const out = new Float32Array(image.width * image.height);
  for (let i = 0; i < out.length; i++) {
    const o = i * 4;
    const alpha = image.data[o + 3] / 255;
    const r = image.data[o] * alpha + 255 * (1 - alpha);
    const g = image.data[o + 1] * alpha + 255 * (1 - alpha);
    const bl = image.data[o + 2] * alpha + 255 * (1 - alpha);
    out[i] = 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  }
  return out;
}

/// Separable box blur, clamped at the edges.
function blur(src, w, h, radius) {
  if (radius <= 0) return src;
  const pass = (input) => {
    const out = new Float32Array(input.length);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        let sum = 0;
        let n = 0;
        for (let k = -radius; k <= radius; k++) {
          const sx = Math.min(w - 1, Math.max(0, x + k));
          sum += input[y * w + sx];
          n++;
        }
        out[y * w + x] = sum / n;
      }
    }
    return out;
  };

  const horizontal = pass(src);
  const out = new Float32Array(src.length);
  for (let x = 0; x < w; x++) {
    for (let y = 0; y < h; y++) {
      let sum = 0;
      let n = 0;
      for (let k = -radius; k <= radius; k++) {
        const sy = Math.min(h - 1, Math.max(0, y + k));
        sum += horizontal[sy * w + x];
        n++;
      }
      out[y * w + x] = sum / n;
    }
  }
  return out;
}

process.exit(failed ? 1 : 0);
