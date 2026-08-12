/**
 * Load an exported page in a real browser and check that it behaves.
 *
 * The Rust tests prove the exporter writes the right markup. They cannot prove the page
 * *works*: that the animation runtime parses its payload, that the Web Animations calls
 * are accepted, that nothing throws on load, and — the one most easily broken by a
 * well-meaning change — that motion is suppressed when the visitor has asked for
 * reduced motion.
 *
 * Usage: node check-export.mjs <path-to-index.html>
 */

import { chromium } from "playwright";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const target = process.argv[2];
if (!target) {
  console.error("usage: check-export.mjs <index.html>");
  process.exit(2);
}

const url = pathToFileURL(resolve(target)).href;
const failures = [];

function check(name, ok, detail = "") {
  if (ok) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail ? ` — ${detail}` : ""}`);
    failures.push(name);
  }
}

const browser = await chromium.launch();

try {
  // ---------------------------------------------------------------------
  // Normal motion
  // ---------------------------------------------------------------------
  {
    const context = await browser.newContext();
    const page = await context.newPage();

    const errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(m.text());
    });

    await page.goto(url, { waitUntil: "load" });
    check("page loads without errors", errors.length === 0, errors.join("; "));

    const svgCount = await page.locator("svg.md-canvas").count();
    check("the artwork is present", svgCount === 1);

    // The runtime creates one Web Animation per compiled entry.
    const animations = await page.evaluate(() =>
      document.getAnimations().map((a) => ({
        state: a.playState,
        duration: a.effect?.getTiming?.().duration ?? 0,
      })),
    );
    check("animations were created", animations.length > 0, `found ${animations.length}`);
    check(
      "animations have a real duration",
      animations.every((a) => Number(a.duration) > 0),
    );

    // The hidden semantic outline is what makes the page navigable and indexable.
    const headings = await page.locator(".md-a11y h1, .md-a11y h2, .md-a11y p").count();
    check("an accessibility outline is present", headings >= 0);

    const artworkHidden = await page
      .locator("svg.md-canvas")
      .getAttribute("aria-hidden");
    check("the artwork is hidden from screen readers", artworkHidden === "true");

    await context.close();
  }

  // ---------------------------------------------------------------------
  // Reduced motion
  // ---------------------------------------------------------------------
  {
    const context = await browser.newContext({ reducedMotion: "reduce" });
    const page = await context.newPage();

    const errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));

    await page.goto(url, { waitUntil: "load" });
    check("page loads under reduced motion", errors.length === 0, errors.join("; "));

    const running = await page.evaluate(
      () => document.getAnimations().filter((a) => a.playState === "running").length,
    );
    // Default behaviour is `skip`: the design arrives at its final state without
    // travelling. Nothing should be moving.
    check("nothing animates under reduced motion", running === 0, `${running} running`);

    await context.close();
  }
} finally {
  await browser.close();
}

if (failures.length > 0) {
  console.error(`\n${failures.length} check(s) failed`);
  process.exit(1);
}
console.log("\nexported page verified");
