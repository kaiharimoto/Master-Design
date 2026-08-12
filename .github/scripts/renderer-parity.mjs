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
 * resvg and Skia rasterize the same geometry with different anti-aliasing, different
 * hinting, and different sub-pixel glyph positioning — and *how* differently depends on
 * the machine: this comparison scored 0.4% on a development container and 3.8% on a CI
 * runner for byte-identical inputs. Compared raw, the check measures the font rasterizer
 * rather than the renderers, and there is no threshold that both passes everywhere and
 * catches anything.
 *
 * Three things fix that, in order of how much they contribute:
 *
 * 1. Chromium is launched with hinting and sub-pixel positioning off, which is what
 *    makes text rendering the same on any machine rather than a property of the host's
 *    fontconfig.
 * 2. Both images are downsampled before comparison. A glyph edge that moved a third of a
 *    pixel disappears; a line that wrapped somewhere else, or a heading a point larger,
 *    displaces ink by whole pixels and survives.
 * 3. The remaining tolerance is deliberately loose per pixel and tight per image, because
 *    the failures worth catching are structural — they move thousands of pixels at once.
 *
 * The calibration to keep honest, measured on the fixture: these settings score **0.08%**
 * for a correct render and **3.18%** for a one-point change to the heading's font size —
 * a change of under 2%. The limit sits between them with an order of magnitude of
 * headroom above the noise, which is what absorbs a rasterizer the check has never run
 * on. Anything that narrows that gap has broken it.
 */
const DOWNSCALE = 6;
const CHANNEL_TOLERANCE = 30;
const MAX_DIFFERENT_FRACTION = 0.012;

// `MD_CHROMIUM` lets a machine that already has a browser point at it rather than
// downloading a second copy — which is the situation in the development container, where
// the pinned revision and the installed one do not match.
const browser = await chromium.launch({
  ...(process.env.MD_CHROMIUM ? { executablePath: process.env.MD_CHROMIUM } : {}),
  // Text rendering that does not depend on the host. Without these, glyph positions and
  // edge treatment come from the machine's fontconfig, and the same page compared against
  // the same snapshot scores an order of magnitude differently on two machines.
  args: [
    "--font-render-hinting=none",
    "--disable-font-subpixel-positioning",
    "--disable-lcd-text",
    "--force-color-profile=srgb",
    "--disable-skia-runtime-opts",
  ],
});
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

    const a = downsample(flatten(browserImage), w, h, DOWNSCALE);
    const b = downsample(flatten(resvgImage), w, h, DOWNSCALE);
    const total = a.length;

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

/**
 * Average `factor × factor` blocks into one value each.
 *
 * The filter that separates "rasterized slightly differently" from "somewhere else". A
 * glyph edge that moved a fraction of a pixel contributes a fraction of a level to one
 * block and vanishes into the tolerance; a line of text that wrapped at a different word
 * moves whole blocks from paper to ink.
 */
function downsample(src, w, h, factor) {
  const ow = Math.ceil(w / factor);
  const oh = Math.ceil(h / factor);
  const out = new Float32Array(ow * oh);

  for (let by = 0; by < oh; by++) {
    for (let bx = 0; bx < ow; bx++) {
      let sum = 0;
      let n = 0;
      for (let y = by * factor; y < Math.min(h, (by + 1) * factor); y++) {
        for (let x = bx * factor; x < Math.min(w, (bx + 1) * factor); x++) {
          sum += src[y * w + x];
          n++;
        }
      }
      out[by * ow + bx] = n > 0 ? sum / n : 0;
    }
  }
  return out;
}

process.exit(failed ? 1 : 0);
